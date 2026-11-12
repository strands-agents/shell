use crate::prelude::*;
use jaq_core::box_iter::box_once;
use jaq_core::load::{Arena, File, Loader};

const HELP: &str = "Usage: jq [OPTIONS] FILTER [FILE]
JSON processor.

Options:
  -r, --raw-output    Output raw strings (no quotes)
  -R, --raw-input     Read each line as a string
  -s, --slurp         Read all inputs into an array
  -c, --compact       Compact output
  -e, --exit-status   Exit with non-zero if last output is false/null
  -n, --null-input    Use null as input
  -j, --join-output   No newline after each output
      --arg NAME VALUE      Bind $NAME to the string VALUE
      --argjson NAME JSON   Bind $NAME to the parsed JSON value";

/// Run jq filter on input, returning formatted output lines or an error.
/// All jaq types (Val, Rc) are confined to this non-async function.
#[allow(clippy::too_many_arguments)]
fn run_filter(
    filter_str: &str,
    input_str: &str,
    null_input: bool,
    raw_input: bool,
    slurp: bool,
    raw_output: bool,
    compact: bool,
    named_args: &[(String, serde_json::Value)],
) -> Result<Vec<String>, String> {
    let loader = Loader::new(jaq_std::defs().chain(jaq_json::defs()));
    let arena = Arena::default();
    let program = File {
        code: filter_str,
        path: (),
    };

    let modules = loader
        .load(&arena, program)
        .map_err(|errs| format!("jq: parse error: {:?}", errs.first()))?;

    // Safety: we use funs() for full jq compatibility but replace
    // dangerous functions that could escape the sandbox:
    // - env: leaks host environment variables (including secrets)
    // - halt/halt_error: calls process::exit(), killing the host
    // We replace them with safe stubs that return errors.
    const BLOCKED: &[&str] = &["env", "halt", "halt_error"];

    let safe_funs = jaq_std::funs()
        .chain(jaq_json::funs())
        .filter(|(name, _, _)| !BLOCKED.contains(name));

    // Provide safe stubs for blocked functions referenced by defs
    let halt_error_stub: jaq_std::Filter<jaq_core::Native<jaq_json::Val>> = (
        "halt_error",
        [jaq_core::Bind::Var(())].into(),
        jaq_core::Native::new(
            |_, _cv: jaq_core::Cv<jaq_json::Val>| -> jaq_core::ValXs<jaq_json::Val> {
                box_once(Err(jaq_core::Exn::from(jaq_core::Error::str(
                    "halt_error is disabled in sandbox",
                ))))
            },
        ),
    );
    let env_stub: jaq_std::Filter<jaq_core::Native<jaq_json::Val>> = (
        "env",
        jaq_std::v(0),
        jaq_core::Native::new(
            |_, _: jaq_core::Cv<jaq_json::Val>| -> jaq_core::ValXs<jaq_json::Val> {
                box_once(Ok(jaq_json::Val::from(serde_json::json!({}))))
            },
        ),
    );

    // Named args become global variables, plus `$ARGS` like real jq.
    let named: serde_json::Map<String, serde_json::Value> = named_args.iter().cloned().collect();
    let args_obj = serde_json::json!({ "positional": [], "named": named });
    let var_names: Vec<String> = named_args
        .iter()
        .map(|(n, _)| format!("${n}"))
        .chain(["$ARGS".to_string()])
        .collect();
    let var_vals: Vec<jaq_json::Val> = named_args
        .iter()
        .map(|(_, v)| jaq_json::Val::from(v.clone()))
        .chain([jaq_json::Val::from(args_obj)])
        .collect();

    let filter = jaq_core::Compiler::default()
        .with_funs(safe_funs.chain([halt_error_stub, env_stub]))
        .with_global_vars(var_names.iter().map(String::as_str))
        .compile(modules)
        .map_err(|errs| format!("jq: compile error: {:?}", errs.first()))?;

    // Parse inputs
    let inputs: Vec<serde_json::Value> = if null_input {
        vec![serde_json::Value::Null]
    } else if raw_input {
        let lines: Vec<serde_json::Value> = input_str
            .lines()
            .map(|l| serde_json::Value::String(l.to_string()))
            .collect();
        if slurp {
            vec![serde_json::Value::Array(lines)]
        } else {
            lines
        }
    } else {
        let trimmed = input_str.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        let mut vals = Vec::new();
        let stream = serde_json::Deserializer::from_str(trimmed).into_iter::<serde_json::Value>();
        for result in stream {
            match result {
                Ok(v) => vals.push(v),
                Err(e) => return Err(format!("jq: parse error: {}", e)),
            }
        }
        if slurp {
            vec![serde_json::Value::Array(vals)]
        } else {
            vals
        }
    };

    let mut output = Vec::new();
    let mut last_json: Option<serde_json::Value> = None;

    for input_json in inputs {
        let input = jaq_json::Val::from(input_json);
        let iter_inputs = jaq_core::RcIter::new(core::iter::empty());
        let out = filter.run((
            jaq_core::Ctx::new(var_vals.iter().cloned(), &iter_inputs),
            input,
        ));

        for result in out {
            match result {
                Ok(val) => {
                    let json: serde_json::Value = val.into();
                    let s = if raw_output {
                        if let Some(s) = json.as_str() {
                            s.to_string()
                        } else if compact {
                            json.to_string()
                        } else {
                            serde_json::to_string_pretty(&json).unwrap_or_default()
                        }
                    } else if compact {
                        json.to_string()
                    } else {
                        serde_json::to_string_pretty(&json).unwrap_or_default()
                    };
                    output.push(s);
                    last_json = Some(json);
                }
                Err(err) => return Err(format!("jq: error: {}", err)),
            }
        }
    }

    // Encode exit_status info as a special marker if needed
    // We'll handle this in the caller
    if let Some(ref v) = last_json {
        if v.is_null() || *v == serde_json::Value::Bool(false) {
            output.push("\x00EXIT_FALSE".to_string());
        }
    } else {
        output.push("\x00EXIT_FALSE".to_string());
    }

    Ok(output)
}

#[command("jq")]
async fn cmd_jq(os: &dyn Kernel, args: &[String]) -> CommandResult {
    let mut parser = lexopt::Parser::from_args(args);
    let mut raw_output = false;
    let mut raw_input = false;
    let mut slurp = false;
    let mut compact = false;
    let mut exit_status = false;
    let mut null_input = false;
    let mut join_output = false;
    let mut filter_str: Option<String> = None;
    let mut files: Vec<String> = Vec::new();
    let mut named_args: Vec<(String, serde_json::Value)> = Vec::new();

    while let Some(arg) = parser.next()? {
        match arg {
            Short('r') | Long("raw-output") => raw_output = true,
            Short('R') | Long("raw-input") => raw_input = true,
            Short('s') | Long("slurp") => slurp = true,
            Short('c') | Long("compact") => compact = true,
            Short('e') | Long("exit-status") => exit_status = true,
            Short('n') | Long("null-input") => null_input = true,
            Short('j') | Long("join-output") => join_output = true,
            Long(opt @ ("arg" | "argjson")) => {
                let is_json = opt == "argjson";
                let (Ok(name), Ok(text)) = (parser.value(), parser.value()) else {
                    let what = if is_json { "argjson" } else { "arg" };
                    let example = if is_json { "text" } else { "value" };
                    let mut w = io::stderr()?;
                    wprintln!(
                        w,
                        "jq: --{what} takes two parameters (e.g. --{what} varname {example})"
                    )?;
                    return Ok(2);
                };
                let name = name.string()?;
                let text = text.string()?;
                // Like jq, the first binding wins without parsing later JSON.
                if named_args.iter().any(|(n, _)| *n == name) {
                    continue;
                }
                let value = if is_json {
                    match serde_json::from_str(&text) {
                        Ok(v) => v,
                        Err(_) => {
                            let mut w = io::stderr()?;
                            wprintln!(w, "jq: invalid JSON text passed to --argjson")?;
                            return Ok(2);
                        }
                    }
                } else {
                    serde_json::Value::String(text)
                };
                named_args.push((name, value));
            }
            Short('h') | Long("help") => {
                let mut w = io::stdout()?;
                wprintln!(w, "{}", HELP)?;
                return Ok(0);
            }
            Value(val) => {
                let s = val.string()?;
                if filter_str.is_none() {
                    filter_str = Some(s);
                } else {
                    files.push(s);
                }
            }
            _ => return Err(arg.unexpected().into()),
        }
    }

    let filter_str = match filter_str {
        Some(f) => f,
        None => {
            let mut w = io::stderr()?;
            wprintln!(w, "jq: no filter given")?;
            return Ok(2);
        }
    };

    // Read input
    let max_output = io::with_process(|p| p.max_output);
    let input_str = if null_input {
        String::new()
    } else if files.is_empty() {
        let mut r = io::stdin()?;
        crate::os::read_to_string_limited(&mut r, max_output).await?
    } else {
        let mut s = String::new();
        for f in &files {
            let fd = io::open(os, f, OpenFlags::read()).await?;
            let mut r = io::take_reader(fd)?;
            s.push_str(&crate::os::read_to_string_limited(&mut r, max_output).await?);
        }
        s
    };

    // Run filter (all jaq types confined to this sync function)
    let result = run_filter(
        &filter_str,
        &input_str,
        null_input,
        raw_input,
        slurp,
        raw_output,
        compact,
        &named_args,
    );

    match result {
        Ok(lines) => {
            let mut w = io::stdout()?;
            let mut saw_exit_false = false;
            for line in &lines {
                if line == "\x00EXIT_FALSE" {
                    saw_exit_false = true;
                    continue;
                }
                wprint!(w, "{}", line)?;
                if !join_output {
                    wprintln!(w)?;
                }
            }
            if exit_status && saw_exit_false {
                return Ok(1);
            }
            Ok(0)
        }
        Err(msg) => {
            let mut e = io::stderr()?;
            wprintln!(e, "{}", msg)?;
            Ok(5)
        }
    }
}
