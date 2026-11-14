//! How `command -V` labels what it finds, and which variables reach the
//! commands the shell runs when they come from the embedder, `cd`, the
//! starting environment, `export` or a prefix assignment. The differential
//! cases cover the parts that can be compared with bash.

use strands_shell::{Output, Shell};

fn run_in(shell: &mut Shell, script: &str) -> Output {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    rt.block_on(local.run_until(shell.run(script)))
}

fn run(script: &str) -> Output {
    run_in(&mut Shell::builder().build().unwrap(), script)
}

fn assert_stdout(script: &str, stdout: &str) {
    let out = run(script);
    assert_eq!(out.status, 0, "{script}: {}", out.stderr);
    assert_eq!(out.stdout, stdout, "{script}");
}

#[test]
fn command_big_v_labels_special_builtins() {
    assert_stdout(
        "command -V export set :",
        "export is a special shell builtin\nset is a special shell builtin\n: is a special shell builtin\n",
    );
}

#[test]
fn command_big_v_labels_builtins_and_registered_commands() {
    assert_stdout(
        "command -V echo cd ls",
        "echo is a shell builtin\ncd is a shell builtin\nls is a shell builtin\n",
    );
}

#[test]
fn command_big_v_labels_functions() {
    assert_stdout("f() { :; }; command -V f", "f is a shell function\n");
}

#[test]
fn command_v_names_a_builtin_shadowed_by_a_function() {
    assert_stdout("echo() { :; }; command -v echo", "echo\n");
}

#[test]
fn command_lookups_fail_for_an_unknown_name() {
    let out = run("command -v no_such_cmd; echo \"v $?\"; command -V no_such_cmd; echo \"V $?\"");
    assert_eq!(out.stdout, "v 1\nV 1\n");
}

#[test]
fn builder_and_set_env_variables_reach_child_commands() {
    let mut shell = Shell::builder().env("FROM_BUILDER", "b").build().unwrap();
    shell.set_env("FROM_SET_ENV", "s");
    let out = run_in(
        &mut shell,
        "env | grep -E '^FROM_(BUILDER|SET_ENV)=' | sort; \
         sh -c 'echo \"sh: $FROM_BUILDER $FROM_SET_ENV\"'; \
         lua -e 'print(\"lua: \" .. os.getenv(\"FROM_BUILDER\") .. \" \" .. os.getenv(\"FROM_SET_ENV\"))'",
    );
    assert_eq!(out.status, 0, "{}", out.stderr);
    assert_eq!(
        out.stdout,
        "FROM_BUILDER=b\nFROM_SET_ENV=s\nsh: b s\nlua: b s\n"
    );
}

#[test]
fn the_starting_environment_reaches_env() {
    let out = run("env | grep -E '^(HOME|PATH|USER|PWD)=' | sed 's/=.*//' | sort");
    assert_eq!(out.stdout, "HOME\nPATH\nPWD\nUSER\n", "{}", out.stderr);
}

#[test]
fn cd_updates_pwd_and_oldpwd_for_child_commands() {
    assert_stdout(
        "cd /tmp; cd /; env | grep -E '^(PWD|OLDPWD)=' | sort",
        "OLDPWD=/tmp\nPWD=/\n",
    );
}

#[test]
fn exported_and_prefix_variables_reach_sh_and_lua() {
    assert_stdout(
        "export E=1; sh -c 'echo \"sh E=$E\"'; P=2 sh -c 'echo \"sh P=$P\"'; \
         lua -e 'print(\"lua E=\" .. os.getenv(\"E\"))'",
        "sh E=1\nsh P=2\nlua E=1\n",
    );
}

#[test]
fn a_function_local_shadow_is_restored_for_child_commands() {
    assert_stdout(
        "export W=outer; f() { local W=inner; sh -c 'echo \"in $W\"'; }; f; sh -c 'echo \"out $W\"'",
        "in inner\nout outer\n",
    );
}
