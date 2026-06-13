//! Shell command parser (recursive descent).
//!
//! The parser preserves variable references, backticks, and quoting
//! context in the AST. Expansion happens at execution time, not parse
//! time — matching how dash/ash work.

/// A part of a word, preserving quoting context for the expander.
#[derive(Debug, PartialEq, Clone)]
pub enum WordPart {
    /// Unquoted or double-quoted literal text.
    Literal(String),
    /// Single-quoted text — no expansion.
    SingleQuoted(String),
    /// `$VAR` or `${VAR}` — expanded at runtime.
    Var(String),
    /// `${var op word}` — parameter expansion with operator.
    /// Fields: (name, operator, word, colon_variant).
    /// Operators: `-`, `=`, `?`, `+`, `%`, `%%`, `#` (trim), `##` (trim).
    /// `${#var}` is represented as operator `"len"`, word empty.
    VarOp(String, String, Vec<WordPart>, bool),
    /// `` `cmd` `` — command substitution, expanded at runtime.
    Backtick(String),
    /// `$(cmd)` — command substitution, expanded at runtime.
    DollarParen(String),
    /// `$((expr))` — arithmetic expansion, evaluated at runtime.
    Arith(String),
    /// `~` or `~user` — tilde expansion.
    Tilde(String),
    /// A double-quoted region containing expandable parts.
    DoubleQuoted(Vec<WordPart>),
}

/// A word is a sequence of parts that get concatenated after expansion.
pub type Word = Vec<WordPart>;

/// A single command with its arguments and optional I/O redirections.
#[derive(Debug, PartialEq, Clone)]
pub struct Command {
    pub env: Vec<(Word, Word)>,
    pub args: Vec<Word>,
    pub redirects: Vec<Redirect>,
}

#[derive(Debug, PartialEq, Clone)]
pub enum Redirect {
    /// `fd>file` (default fd=1)
    Write(u32, Word),
    /// `fd>>file` (default fd=1)
    Append(u32, Word),
    /// `fd<file` (default fd=0)
    Read(u32, Word),
    /// `fd<>file` (default fd=0)
    ReadWrite(u32, Word),
    /// `fd>|file` (default fd=1)
    Clobber(u32, Word),
    /// `fd>&target` (default fd=1)
    DupWrite(u32, Word),
    /// `fd<&target` (default fd=0)
    DupRead(u32, Word),
    /// `fd<<DELIM` / `fd<<-DELIM` — here-document (fd, delimiter, body, strip_tabs, quoted)
    HereDoc(u32, String, String, bool, bool),
}

/// A pipeline is a sequence of commands connected by pipes.
pub type Pipeline = Vec<Command>;

/// How pipelines are chained together.
#[derive(Debug, PartialEq, Clone)]
pub enum Connector {
    Semi,
    And,
    Or,
    Background,
}

/// An item in a command line.
#[derive(Debug, PartialEq, Clone)]
pub enum Item {
    Pipeline(Pipeline, bool), // (pipeline, negated)
    /// A compound command (group, subshell, if, while, for, case) piped into
    /// a trailing pipeline: `for i in ...; do ...; done | sort | head`
    CompoundPipeline {
        compound: Box<Item>,
        tail: Pipeline,
        negated: bool,
    },
    /// A compound command with redirections: `while read LINE; do ...; done < file`
    CompoundRedirect {
        item: Box<Item>,
        redirects: Vec<Redirect>,
    },
    Group(CommandLine),
    Subshell(CommandLine),
    If {
        /// (condition, body) pairs: first is `if`, rest are `elif`
        branches: Vec<(CommandLine, CommandLine)>,
        else_body: Option<CommandLine>,
    },
    While {
        condition: CommandLine,
        body: CommandLine,
    },
    Until {
        condition: CommandLine,
        body: CommandLine,
    },
    For {
        var: String,
        words: Vec<Word>,
        body: CommandLine,
    },
    Case {
        word: Word,
        arms: Vec<CaseArm>,
    },
    Function {
        name: String,
        body: CommandLine,
    },
}

/// A single arm in a case statement: patterns and body.
#[derive(Debug, PartialEq, Clone)]
pub struct CaseArm {
    pub patterns: Vec<Word>,
    pub body: CommandLine,
}

/// A sequence of items with connectors between them.
pub type CommandLine = Vec<(Item, Option<Connector>)>;

// Reserved words that terminate a command line when in command position.
const RESERVED: &[&str] = &[
    "then", "elif", "else", "fi", "do", "done", "{", "}", "esac", ";;",
];

fn is_reserved(tok: &Token) -> bool {
    matches!(tok, Token::Word(w) if RESERVED.contains(&w.as_str()))
}

/// Return the plain text of a word (for reserved word matching).
/// Only works for simple literal words.
pub fn word_to_str(word: &Word) -> Option<String> {
    let mut s = String::new();
    for part in word {
        match part {
            WordPart::Literal(t) => s.push_str(t),
            WordPart::SingleQuoted(t) => s.push_str(t),
            _ => return None,
        }
    }
    Some(s)
}

/// Parse a full input line into a command line.
pub fn parse(input: &str) -> Result<CommandLine, String> {
    parse_with_aliases(input, &mut |_| None, &std::collections::HashMap::new())
}

/// Parse with a line reader for here-documents.
/// `read_line` is called with the delimiter and should return the next line, or None on EOF.
pub fn parse_with_reader(
    input: &str,
    read_line: &mut dyn FnMut(&str) -> Option<String>,
) -> Result<CommandLine, String> {
    parse_with_aliases(input, read_line, &std::collections::HashMap::new())
}

/// Parse with alias expansion and a line reader for here-documents.
pub fn parse_with_aliases(
    input: &str,
    read_line: &mut dyn FnMut(&str) -> Option<String>,
    aliases: &std::collections::HashMap<String, String>,
) -> Result<CommandLine, String> {
    let mut tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Ok(vec![]);
    }
    if !aliases.is_empty() {
        expand_aliases(&mut tokens, aliases);
    }
    let (mut cl, rest) = parse_command_line(&tokens, &[])?;
    if let Some(tok) = rest.first() {
        return Err(format!("unexpected token: {}", tok_name(tok)));
    }
    // Resolve pending here-doc bodies
    resolve_heredocs(&mut cl, read_line)?;
    Ok(cl)
}

/// Expand aliases in command position within the token stream.
/// POSIX rules: aliases expand only in command position (first word of a simple command).
/// If an alias value ends with a space, the next word is also checked for alias expansion.
fn expand_aliases(tokens: &mut Vec<Token>, aliases: &std::collections::HashMap<String, String>) {
    let mut i = 0;
    let mut cmd_pos = true;

    while i < tokens.len() {
        if cmd_pos {
            let mut seen = std::collections::HashSet::new();
            let mut trail_space = false;
            let before_len = tokens.len();
            // Repeatedly expand the token at position i until no more alias matches
            loop {
                if i >= tokens.len() {
                    break;
                }
                if let Token::Word(ref name) = tokens[i]
                    && !is_reserved(&tokens[i])
                    && !seen.contains(name.as_str())
                    && let Some(val) = aliases.get(name.as_str())
                {
                    trail_space = val.ends_with(' ') || val.ends_with('\t');
                    seen.insert(name.clone());
                    if let Ok(mut expanded) = tokenize(val) {
                        tokens.remove(i);
                        for (j, tok) in expanded.drain(..).enumerate() {
                            tokens.insert(i + j, tok);
                        }
                        continue; // re-check position i
                    }
                }
                break;
            }
            if !seen.is_empty() {
                let expanded_count = tokens.len() - before_len + 1;
                i += expanded_count;
                cmd_pos = trail_space;
                continue;
            }
        }

        cmd_pos = matches!(
            &tokens[i],
            Token::Semi | Token::And | Token::Or | Token::Pipe | Token::Amp | Token::LParen
        ) || matches!(&tokens[i], Token::Word(w) if matches!(w.as_str(),
            "if" | "then" | "else" | "elif" | "while" | "until" | "do" | "{" | "!"
        ));
        i += 1;
    }
}

/// Walk the AST and fill in here-doc bodies by reading lines from the reader.
fn resolve_heredocs(
    cl: &mut CommandLine,
    read_line: &mut dyn FnMut(&str) -> Option<String>,
) -> Result<(), String> {
    for (item, _) in cl.iter_mut() {
        match item {
            Item::Pipeline(pipeline, _) => {
                for cmd in pipeline.iter_mut() {
                    for redir in &mut cmd.redirects {
                        if let Redirect::HereDoc(_, delim, body, strip, _) = redir {
                            *body = read_heredoc_body(delim, *strip, read_line)?;
                        }
                    }
                }
            }
            Item::Group(inner) | Item::Subshell(inner) => resolve_heredocs(inner, read_line)?,
            Item::If {
                branches,
                else_body,
            } => {
                for (cond, body) in branches {
                    resolve_heredocs(cond, read_line)?;
                    resolve_heredocs(body, read_line)?;
                }
                if let Some(eb) = else_body {
                    resolve_heredocs(eb, read_line)?;
                }
            }
            Item::While { condition, body } | Item::Until { condition, body } => {
                resolve_heredocs(condition, read_line)?;
                resolve_heredocs(body, read_line)?;
            }
            Item::For { body, .. } => resolve_heredocs(body, read_line)?,
            Item::Case { arms, .. } => {
                for arm in arms {
                    resolve_heredocs(&mut arm.body, read_line)?;
                }
            }
            Item::Function { body, .. } => resolve_heredocs(body, read_line)?,
            Item::CompoundPipeline { compound, tail, .. } => {
                let mut inner_cl = vec![(*compound.clone(), None)];
                resolve_heredocs(&mut inner_cl, read_line)?;
                **compound = inner_cl.into_iter().next().unwrap().0;
                for cmd in tail.iter_mut() {
                    for redir in &mut cmd.redirects {
                        if let Redirect::HereDoc(_, delim, body, strip, _) = redir {
                            *body = read_heredoc_body(delim, *strip, read_line)?;
                        }
                    }
                }
            }
            Item::CompoundRedirect { item, redirects } => {
                let mut inner_cl = vec![(*item.clone(), None)];
                resolve_heredocs(&mut inner_cl, read_line)?;
                **item = inner_cl.into_iter().next().unwrap().0;
                for redir in redirects.iter_mut() {
                    if let Redirect::HereDoc(_, delim, body, strip, _) = redir {
                        *body = read_heredoc_body(delim, *strip, read_line)?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn read_heredoc_body(
    delim: &str,
    strip: bool,
    read_line: &mut dyn FnMut(&str) -> Option<String>,
) -> Result<String, String> {
    let mut body = String::new();
    while let Some(line) = read_line(delim) {
        let check = if strip {
            line.trim_start_matches('\t')
        } else {
            &line
        };
        if check.trim_end_matches('\n').trim_end_matches('\r') == delim {
            break;
        }
        if strip {
            body.push_str(check);
        } else {
            body.push_str(&line);
        }
        body.push('\n');
    }
    Ok(body)
}

#[derive(Debug, PartialEq, Clone)]
enum Token {
    Word(String),    // plain text for matching reserved words / operators
    WordParts(Word), // rich word with quoting/expansion info
    Pipe,
    Semi,
    DoubleSemi, // ;;
    And,
    Or,
    /// `>` or `N>` (fd stored in Redirect during build_pipeline)
    RedirectOut,
    /// `>>` or `N>>`
    RedirectAppend,
    /// `<` or `N<`
    RedirectIn,
    /// `<<`
    HereDoc,
    /// `<<-`
    HereDocStrip,
    /// `>|` or `N>|`
    RedirectClobber,
    /// `<>` or `N<>`
    RedirectReadWrite,
    /// `>&` or `N>&`
    DupOut,
    /// `<&` or `N<&`
    DupIn,
    LParen,
    RParen,
    Amp,
}

fn tok_name(tok: &Token) -> &str {
    match tok {
        Token::Word(w) => w,
        Token::WordParts(_) => "<word>",
        Token::Pipe => "|",
        Token::Semi => ";",
        Token::DoubleSemi => ";;",
        Token::And => "&&",
        Token::Or => "||",
        Token::RedirectOut => ">",
        Token::RedirectAppend => ">>",
        Token::RedirectIn => "<",
        Token::HereDoc => "<<",
        Token::HereDocStrip => "<<-",
        Token::RedirectClobber => ">|",
        Token::RedirectReadWrite => "<>",
        Token::DupOut => ">&",
        Token::DupIn => "<&",
        Token::LParen => "(",
        Token::RParen => ")",
        Token::Amp => "&",
    }
}

/// Collect a $VAR or ${VAR} name from the char stream. Returns the var name.
fn collect_var(
    chars: &mut std::iter::Peekable<std::str::Chars>,
) -> Result<Option<WordPart>, String> {
    if chars.peek() == Some(&'{') {
        chars.next();
        // ${#var} — length
        if chars.peek() == Some(&'#') {
            let mut lookahead = chars.clone();
            lookahead.next(); // skip #
            // If next is } or alphanumeric/_, it's ${#var}
            if let Some(&ch) = lookahead.peek() {
                if ch == '}' {
                    // ${#} — number of positional params
                    chars.next(); // skip #
                    chars.next(); // skip }
                    return Ok(Some(WordPart::Var("#".into())));
                }
                if ch.is_ascii_alphanumeric() || ch == '_' || "?$!@*".contains(ch) {
                    chars.next(); // skip #
                    let name = collect_var_name(chars);
                    if chars.peek() == Some(&'}') {
                        chars.next();
                    }
                    return Ok(Some(WordPart::VarOp(name, "len".into(), vec![], false)));
                }
            }
        }
        let name = collect_var_name(chars);
        // Check for operator
        match chars.peek() {
            Some(&'}') => {
                chars.next();
                Ok(Some(WordPart::Var(name)))
            }
            Some(&':') => {
                chars.next();
                match chars.peek() {
                    Some(&'-') | Some(&'=') | Some(&'?') | Some(&'+') => {
                        let op = chars.next().unwrap().to_string();
                        let word = collect_brace_word(chars)?;
                        Ok(Some(WordPart::VarOp(name, op, word, true)))
                    }
                    _ => {
                        // bare colon — not a valid operator, treat as name
                        Ok(Some(WordPart::Var(name)))
                    }
                }
            }
            Some(&'-') | Some(&'=') | Some(&'?') | Some(&'+') => {
                let op = chars.next().unwrap().to_string();
                let word = collect_brace_word(chars)?;
                Ok(Some(WordPart::VarOp(name, op, word, false)))
            }
            Some(&'%') => {
                chars.next();
                let op = if chars.peek() == Some(&'%') {
                    chars.next();
                    "%%"
                } else {
                    "%"
                }
                .to_string();
                let word = collect_brace_word(chars)?;
                Ok(Some(WordPart::VarOp(name, op, word, false)))
            }
            Some(&'#') => {
                chars.next();
                let op = if chars.peek() == Some(&'#') {
                    chars.next();
                    "##"
                } else {
                    "#"
                }
                .to_string();
                let word = collect_brace_word(chars)?;
                Ok(Some(WordPart::VarOp(name, op, word, false)))
            }
            _ => {
                // No closing brace found — consume to } anyway
                while let Some(&ch) = chars.peek() {
                    if ch == '}' {
                        chars.next();
                        break;
                    }
                    chars.next();
                }
                Ok(Some(WordPart::Var(name)))
            }
        }
    } else {
        // Special single-char variables: $?, $$, $!, $#, $-, $0-$9
        if let Some(&ch) = chars.peek()
            && "?$!#-0123456789@*".contains(ch)
        {
            chars.next();
            return Ok(Some(WordPart::Var(ch.to_string())));
        }
        let mut name = String::new();
        while let Some(&ch) = chars.peek() {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                name.push(ch);
                chars.next();
            } else {
                break;
            }
        }
        if name.is_empty() {
            Ok(None)
        } else {
            Ok(Some(WordPart::Var(name)))
        }
    }
}

/// Collect just the variable name part inside ${...}.
fn collect_var_name(chars: &mut std::iter::Peekable<std::str::Chars>) -> String {
    // Special single-char variables
    if let Some(&ch) = chars.peek() {
        if "?$!#-@*".contains(ch) {
            chars.next();
            return ch.to_string();
        }
        if ch.is_ascii_digit() {
            chars.next();
            return ch.to_string();
        }
    }
    let mut name = String::new();
    while let Some(&ch) = chars.peek() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            name.push(ch);
            chars.next();
        } else {
            break;
        }
    }
    name
}

/// Collect the word inside ${var op WORD} up to the closing `}`.
/// Handles nested ${...}, quotes, and escapes.
fn collect_brace_word(
    chars: &mut std::iter::Peekable<std::str::Chars>,
) -> Result<Vec<WordPart>, String> {
    let mut parts = Vec::new();
    let mut lit = String::new();
    let mut depth = 1; // we're inside one ${
    while let Some(&ch) = chars.peek() {
        if ch == '}' {
            depth -= 1;
            if depth == 0 {
                chars.next();
                break;
            }
            lit.push(ch);
            chars.next();
        } else if ch == '$' {
            chars.next();
            match collect_dollar(chars)? {
                Some(part) => {
                    if !lit.is_empty() {
                        parts.push(WordPart::Literal(std::mem::take(&mut lit)));
                    }
                    if matches!(&part, WordPart::VarOp(..)) {
                        depth += 0;
                    } // nested ${ already consumed }
                    parts.push(part);
                }
                None => lit.push('$'),
            }
        } else if ch == '\\' {
            chars.next();
            if let Some(&next) = chars.peek() {
                lit.push(next);
                chars.next();
            }
        } else if ch == '\'' {
            chars.next();
            let mut sq = String::new();
            loop {
                match chars.next() {
                    Some('\'') => break,
                    Some(c) => sq.push(c),
                    None => return Err("unterminated single quote".into()),
                }
            }
            if !lit.is_empty() {
                parts.push(WordPart::Literal(std::mem::take(&mut lit)));
            }
            parts.push(WordPart::SingleQuoted(sq));
        } else if ch == '"' {
            chars.next();
            // Simplified: collect as literal for now
            let mut dq = String::new();
            loop {
                match chars.next() {
                    Some('"') => break,
                    Some('\\') => match chars.next() {
                        Some(c @ ('$' | '`' | '"' | '\\')) => dq.push(c),
                        Some(c) => {
                            dq.push('\\');
                            dq.push(c);
                        }
                        None => return Err("unterminated double quote".into()),
                    },
                    Some(c) => dq.push(c),
                    None => return Err("unterminated double quote".into()),
                }
            }
            lit.push_str(&dq);
        } else if ch == '`' {
            chars.next();
            if !lit.is_empty() {
                parts.push(WordPart::Literal(std::mem::take(&mut lit)));
            }
            let mut cmd = String::new();
            loop {
                match chars.next() {
                    Some('`') => break,
                    Some(c) => cmd.push(c),
                    None => return Err("unterminated backtick".into()),
                }
            }
            parts.push(WordPart::Backtick(cmd));
        } else {
            lit.push(ch);
            chars.next();
        }
    }
    if !lit.is_empty() {
        parts.push(WordPart::Literal(lit));
    }
    Ok(parts)
}

/// Collect a $(...) command substitution. Assumes the opening `(` has been consumed.
/// Handles nested parentheses.
fn collect_dollar_paren(
    chars: &mut std::iter::Peekable<std::str::Chars>,
) -> Result<String, String> {
    let mut cmd = String::new();
    let mut depth = 1;
    loop {
        match chars.next() {
            Some('(') => {
                depth += 1;
                cmd.push('(');
            }
            Some(')') => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                cmd.push(')');
            }
            Some(c) => cmd.push(c),
            None => return Err("unterminated $()".into()),
        }
    }
    Ok(cmd)
}

/// Collect a $VAR, ${VAR}, ${VAR op WORD}, or $(cmd) from the char stream.
/// Returns a WordPart or None (bare $).
fn collect_dollar(
    chars: &mut std::iter::Peekable<std::str::Chars>,
) -> Result<Option<WordPart>, String> {
    if chars.peek() == Some(&'(') {
        chars.next();
        if chars.peek() == Some(&'(') {
            // $((expr)) — arithmetic expansion
            chars.next();
            let mut expr = String::new();
            let mut depth = 0;
            loop {
                match chars.next() {
                    Some('(') => {
                        depth += 1;
                        expr.push('(');
                    }
                    Some(')') if depth > 0 => {
                        depth -= 1;
                        expr.push(')');
                    }
                    Some(')') => {
                        // expect second closing )
                        match chars.next() {
                            Some(')') => break,
                            _ => return Err("expected '))'".into()),
                        }
                    }
                    Some(c) => expr.push(c),
                    None => return Err("unterminated $(())".into()),
                }
            }
            Ok(Some(WordPart::Arith(expr)))
        } else {
            let cmd = collect_dollar_paren(chars)?;
            Ok(Some(WordPart::DollarParen(cmd)))
        }
    } else {
        collect_var(chars)
    }
}

/// Public version of collect_dollar for use by the executor (here-doc expansion).
pub fn collect_dollar_pub(
    chars: &mut std::iter::Peekable<std::str::Chars>,
) -> Result<Option<WordPart>, String> {
    collect_dollar(chars)
}

fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();

    while let Some(&c) = chars.peek() {
        match c {
            '#' => {
                // comment — skip rest of line
                while let Some(&c) = chars.peek() {
                    if c == '\n' {
                        break;
                    }
                    chars.next();
                }
            }
            '\n' => {
                chars.next();
                // Emit semicolon for newline as command separator
                // but collapse consecutive newlines / trailing newlines
                // and don't emit after keywords where newlines are not separators
                if !tokens.is_empty() {
                    match tokens.last() {
                        Some(Token::Semi) | Some(Token::And) | Some(Token::Or)
                        | Some(Token::Pipe) | Some(Token::Amp) => {}
                        Some(Token::DoubleSemi) => {}
                        Some(Token::LParen) => {}
                        Some(Token::Word(w))
                            if matches!(
                                w.as_str(),
                                "in" | "do" | "then" | "else" | "elif" | "{" | "!"
                            ) => {}
                        _ => tokens.push(Token::Semi),
                    }
                }
            }
            ' ' | '\t' | '\r' => {
                chars.next();
            }
            '(' => {
                chars.next();
                tokens.push(Token::LParen);
            }
            ')' => {
                chars.next();
                tokens.push(Token::RParen);
            }
            '|' => {
                chars.next();
                if chars.peek() == Some(&'|') {
                    chars.next();
                    tokens.push(Token::Or);
                } else {
                    tokens.push(Token::Pipe);
                }
            }
            '&' => {
                chars.next();
                if chars.peek() == Some(&'&') {
                    chars.next();
                    tokens.push(Token::And);
                } else {
                    tokens.push(Token::Amp);
                }
            }
            ';' => {
                chars.next();
                if chars.peek() == Some(&';') {
                    chars.next();
                    tokens.push(Token::DoubleSemi);
                } else {
                    tokens.push(Token::Semi);
                }
            }
            '>' => {
                chars.next();
                match chars.peek() {
                    Some(&'>') => {
                        chars.next();
                        tokens.push(Token::RedirectAppend);
                    }
                    Some(&'|') => {
                        chars.next();
                        tokens.push(Token::RedirectClobber);
                    }
                    Some(&'&') => {
                        chars.next();
                        tokens.push(Token::DupOut);
                    }
                    _ => tokens.push(Token::RedirectOut),
                }
            }
            '<' => {
                chars.next();
                match chars.peek() {
                    Some(&'<') => {
                        chars.next();
                        if chars.peek() == Some(&'-') {
                            chars.next();
                            tokens.push(Token::HereDocStrip);
                        } else {
                            tokens.push(Token::HereDoc);
                        }
                    }
                    Some(&'>') => {
                        chars.next();
                        tokens.push(Token::RedirectReadWrite);
                    }
                    Some(&'&') => {
                        chars.next();
                        tokens.push(Token::DupIn);
                    }
                    _ => tokens.push(Token::RedirectIn),
                }
            }
            _ => {
                // Collect a word (may span quotes, vars, backticks)
                let mut parts: Word = Vec::new();
                let mut literal = String::new();

                // Tilde expansion: ~ at start of word
                if c == '~' {
                    chars.next();
                    let mut user = String::new();
                    while let Some(&ch) = chars.peek() {
                        if ch.is_ascii_alphanumeric()
                            || ch == '_'
                            || ch == '-'
                            || ch == '.'
                            || ch == '+'
                        {
                            user.push(ch);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    parts.push(WordPart::Tilde(user));
                }

                while let Some(&ch) = chars.peek() {
                    if " \t\n\r|;&<>()#".contains(ch) {
                        break;
                    }
                    match ch {
                        '\\' => {
                            chars.next();
                            if let Some(&next) = chars.peek() {
                                if next == '\n' {
                                    chars.next();
                                }
                                // line continuation
                                else {
                                    literal.push(next);
                                    chars.next();
                                }
                            }
                        }
                        '\'' => {
                            if !literal.is_empty() {
                                parts.push(WordPart::Literal(std::mem::take(&mut literal)));
                            }
                            chars.next();
                            let mut sq = String::new();
                            loop {
                                match chars.next() {
                                    Some('\'') => break,
                                    Some(c) => sq.push(c),
                                    None => return Err("unterminated single quote".into()),
                                }
                            }
                            parts.push(WordPart::SingleQuoted(sq));
                        }
                        '"' => {
                            if !literal.is_empty() {
                                parts.push(WordPart::Literal(std::mem::take(&mut literal)));
                            }
                            chars.next();
                            let mut dq_parts: Vec<WordPart> = Vec::new();
                            let mut dq_lit = String::new();
                            loop {
                                match chars.next() {
                                    Some('"') => break,
                                    Some('\\') => match chars.next() {
                                        Some(c @ ('$' | '`' | '"' | '\\')) => dq_lit.push(c),
                                        Some('\n') => {} // line continuation
                                        Some(c) => {
                                            dq_lit.push('\\');
                                            dq_lit.push(c);
                                        }
                                        None => return Err("unterminated escape".into()),
                                    },
                                    Some('$') => match collect_dollar(&mut chars)? {
                                        Some(part) => {
                                            if !dq_lit.is_empty() {
                                                dq_parts.push(WordPart::Literal(std::mem::take(
                                                    &mut dq_lit,
                                                )));
                                            }
                                            dq_parts.push(part);
                                        }
                                        None => dq_lit.push('$'),
                                    },
                                    Some('`') => {
                                        if !dq_lit.is_empty() {
                                            dq_parts.push(WordPart::Literal(std::mem::take(
                                                &mut dq_lit,
                                            )));
                                        }
                                        let mut cmd = String::new();
                                        loop {
                                            match chars.next() {
                                                Some('`') => break,
                                                Some(c) => cmd.push(c),
                                                None => return Err("unterminated backtick".into()),
                                            }
                                        }
                                        dq_parts.push(WordPart::Backtick(cmd));
                                    }
                                    Some(c) => dq_lit.push(c),
                                    None => return Err("unterminated double quote".into()),
                                }
                            }
                            if !dq_lit.is_empty() {
                                dq_parts.push(WordPart::Literal(dq_lit));
                            }
                            parts.push(WordPart::DoubleQuoted(dq_parts));
                        }
                        '$' => {
                            chars.next();
                            match collect_dollar(&mut chars)? {
                                Some(part) => {
                                    if !literal.is_empty() {
                                        parts.push(WordPart::Literal(std::mem::take(&mut literal)));
                                    }
                                    parts.push(part);
                                }
                                None => literal.push('$'),
                            }
                        }
                        '`' => {
                            chars.next();
                            if !literal.is_empty() {
                                parts.push(WordPart::Literal(std::mem::take(&mut literal)));
                            }
                            let mut cmd = String::new();
                            loop {
                                match chars.next() {
                                    Some('`') => break,
                                    Some(c) => cmd.push(c),
                                    None => return Err("unterminated backtick".into()),
                                }
                            }
                            parts.push(WordPart::Backtick(cmd));
                        }
                        _ => {
                            literal.push(ch);
                            chars.next();
                        }
                    }
                }
                if !literal.is_empty() {
                    parts.push(WordPart::Literal(literal));
                }

                // For reserved word matching, also store the plain text form.
                // Only use Token::Word for purely literal words (no quotes).
                let all_literal = parts.iter().all(|p| matches!(p, WordPart::Literal(_)));
                if all_literal {
                    let s = parts
                        .iter()
                        .map(|p| match p {
                            WordPart::Literal(s) => s.as_str(),
                            _ => "",
                        })
                        .collect::<String>();
                    tokens.push(Token::Word(s));
                } else {
                    tokens.push(Token::WordParts(parts));
                }
            }
        }
    }

    Ok(tokens)
}

/// Get the Word representation from a token.
fn tok_to_word(tok: &Token) -> Word {
    match tok {
        Token::Word(s) => vec![WordPart::Literal(s.clone())],
        Token::WordParts(w) => w.clone(),
        _ => panic!("expected word token"),
    }
}

/// Check if token is a word (either plain or parts).
fn is_word_token(tok: &Token) -> bool {
    matches!(tok, Token::Word(_) | Token::WordParts(_))
}

/// Check if we should stop parsing at this token.
fn is_stop(tok: &Token, stop: &[&str], at_command_start: bool) -> bool {
    match tok {
        Token::RParen => stop.contains(&")"),
        Token::DoubleSemi => stop.contains(&";;"),
        Token::Word(w) if at_command_start => stop.contains(&w.as_str()),
        _ => false,
    }
}

/// If the next token after a compound command is `|`, collect the trailing
/// pipeline commands and wrap everything in a `CompoundPipeline`.
fn collect_compound_pipe(
    tokens: &[Token],
    i: &mut usize,
    _pipeline_tokens: &mut Vec<&Token>,
    item: Item,
    negated: bool,
) -> Result<Item, String> {
    // Collect any redirects after the compound command (e.g. `done < file`)
    let mut redirects: Vec<Redirect> = Vec::new();
    while *i < tokens.len() {
        let fd_prefix: Option<u32> = if let Token::Word(w) = &tokens[*i] {
            if w.len() == 1
                && w.as_bytes()[0].is_ascii_digit()
                && *i + 1 < tokens.len()
                && is_redirect_token(&tokens[*i + 1])
            {
                let fd = (w.as_bytes()[0] - b'0') as u32;
                *i += 1;
                Some(fd)
            } else {
                break;
            }
        } else if is_redirect_token(&tokens[*i]) {
            None
        } else {
            break;
        };
        let tok = tokens[*i].clone();
        *i += 1;
        if *i >= tokens.len() || !is_word_token(&tokens[*i]) {
            return Err(format!("expected filename after '{}'", tok_name(&tok)));
        }
        let word = tok_to_word(&tokens[*i]);
        *i += 1;
        let redir = match tok {
            Token::RedirectOut => Redirect::Write(fd_prefix.unwrap_or(1), word),
            Token::RedirectAppend => Redirect::Append(fd_prefix.unwrap_or(1), word),
            Token::RedirectClobber => Redirect::Clobber(fd_prefix.unwrap_or(1), word),
            Token::RedirectIn => Redirect::Read(fd_prefix.unwrap_or(0), word),
            Token::RedirectReadWrite => Redirect::ReadWrite(fd_prefix.unwrap_or(0), word),
            Token::DupOut => Redirect::DupWrite(fd_prefix.unwrap_or(1), word),
            Token::DupIn => Redirect::DupRead(fd_prefix.unwrap_or(0), word),
            Token::HereDoc => {
                let delim = word_to_str(&word).unwrap_or_default();
                let quoted = word
                    .iter()
                    .any(|p| matches!(p, WordPart::SingleQuoted(_) | WordPart::DoubleQuoted(_)));
                Redirect::HereDoc(fd_prefix.unwrap_or(0), delim, String::new(), false, quoted)
            }
            Token::HereDocStrip => {
                let delim = word_to_str(&word).unwrap_or_default();
                let quoted = word
                    .iter()
                    .any(|p| matches!(p, WordPart::SingleQuoted(_) | WordPart::DoubleQuoted(_)));
                Redirect::HereDoc(fd_prefix.unwrap_or(0), delim, String::new(), true, quoted)
            }
            _ => break,
        };
        redirects.push(redir);
    }

    let item = if !redirects.is_empty() {
        Item::CompoundRedirect {
            item: Box::new(item),
            redirects,
        }
    } else {
        item
    };

    if *i < tokens.len() && tokens[*i] == Token::Pipe {
        // Collect trailing pipeline tokens after the pipe
        *i += 1;
        let mut tail_tokens: Vec<&Token> = Vec::new();
        while *i < tokens.len() {
            match &tokens[*i] {
                Token::Semi | Token::And | Token::Or | Token::Amp => break,
                t if is_reserved(t) => break,
                Token::LParen | Token::RParen => break,
                _ => {
                    tail_tokens.push(&tokens[*i]);
                    *i += 1;
                }
            }
        }
        if tail_tokens.is_empty() {
            return Err("expected command after '|'".into());
        }
        let tail = build_pipeline(&tail_tokens)?;
        let inner = match item {
            Item::CompoundRedirect {
                item: inner,
                redirects,
            } => Item::CompoundRedirect {
                item: inner,
                redirects,
            },
            other => other,
        };
        Ok(Item::CompoundPipeline {
            compound: Box::new(inner),
            tail,
            negated,
        })
    } else {
        Ok(item)
    }
}

/// Parse a command line, stopping at any token in `stop`.
/// Stop words match reserved words at command position, and ")" matches RParen.
/// Returns the parsed command line and remaining tokens (including the stop token).
fn parse_command_line<'a>(
    tokens: &'a [Token],
    stop: &[&str],
) -> Result<(CommandLine, &'a [Token]), String> {
    let mut result: CommandLine = Vec::new();
    let mut pipeline_tokens: Vec<&Token> = Vec::new();
    let mut negated = false;
    let mut i = 0;

    // Skip leading semicolons (from newlines before first command)
    while i < tokens.len() && tokens[i] == Token::Semi {
        i += 1;
    }

    while i < tokens.len() {
        let at_start = pipeline_tokens.is_empty() && result.last().is_none_or(|(_, c)| c.is_some());

        if is_stop(&tokens[i], stop, at_start) {
            break;
        }

        // Pipeline negation: `! pipeline`
        if at_start && matches!(&tokens[i], Token::Word(w) if w == "!") {
            negated = true;
            i += 1;
            continue;
        }

        // Check for unexpected reserved words at command position
        if at_start && is_reserved(&tokens[i]) {
            // Handle { ... } group
            if matches!(&tokens[i], Token::Word(w) if w == "{") {
                let (group, rest) = parse_command_line(&tokens[i + 1..], &["}"])?;
                let rest = expect_word(rest, "}")?;
                i = tokens.len() - rest.len();
                let item = Item::Group(group);
                let item =
                    collect_compound_pipe(tokens, &mut i, &mut pipeline_tokens, item, negated)?;
                negated = false;
                result.push((item, None));
                continue;
            }
            return Err(format!("unexpected '{}'", tok_name(&tokens[i])));
        }

        match &tokens[i] {
            Token::RParen => return Err("unexpected ')'".into()),
            // Function definition: name() { body; }
            Token::Word(name)
                if at_start
                    && i + 2 < tokens.len()
                    && tokens[i + 1] == Token::LParen
                    && tokens[i + 2] == Token::RParen =>
            {
                let fname = name.clone();
                i += 3; // skip name ( )
                let rest = expect_word(&tokens[i..], "{")?;
                let (body, rest) = parse_command_line(rest, &["}"])?;
                let rest = expect_word(rest, "}")?;
                i = tokens.len() - rest.len();
                result.push((Item::Function { name: fname, body }, None));
                continue;
            }
            Token::LParen if at_start => {
                let (group, rest) = parse_command_line(&tokens[i + 1..], &[")"])?;
                let rest = expect_rparen(rest)?;
                if group.is_empty() {
                    return Err("empty group".into());
                }
                i = tokens.len() - rest.len();
                let item = Item::Subshell(group);
                let item =
                    collect_compound_pipe(tokens, &mut i, &mut pipeline_tokens, item, negated)?;
                negated = false;
                result.push((item, None));
                continue;
            }
            Token::Word(w) if w == "if" && at_start => {
                let (item, rest) = parse_if(&tokens[i + 1..])?;
                i = tokens.len() - rest.len();
                let item =
                    collect_compound_pipe(tokens, &mut i, &mut pipeline_tokens, item, negated)?;
                negated = false;
                result.push((item, None));
                continue;
            }
            Token::Word(w) if (w == "while" || w == "until") && at_start => {
                let is_until = w == "until";
                let (item, rest) = parse_while_until(&tokens[i + 1..], is_until)?;
                i = tokens.len() - rest.len();
                let item =
                    collect_compound_pipe(tokens, &mut i, &mut pipeline_tokens, item, negated)?;
                negated = false;
                result.push((item, None));
                continue;
            }
            Token::Word(w) if w == "for" && at_start => {
                let (item, rest) = parse_for(&tokens[i + 1..])?;
                i = tokens.len() - rest.len();
                let item =
                    collect_compound_pipe(tokens, &mut i, &mut pipeline_tokens, item, negated)?;
                negated = false;
                result.push((item, None));
                continue;
            }
            Token::Word(w) if w == "case" && at_start => {
                let (item, rest) = parse_case(&tokens[i + 1..])?;
                i = tokens.len() - rest.len();
                let item =
                    collect_compound_pipe(tokens, &mut i, &mut pipeline_tokens, item, negated)?;
                negated = false;
                result.push((item, None));
                continue;
            }
            Token::Semi | Token::And | Token::Or | Token::Amp => {
                let connector = match &tokens[i] {
                    Token::Semi => Connector::Semi,
                    Token::And => Connector::And,
                    Token::Or => Connector::Or,
                    Token::Amp => Connector::Background,
                    _ => unreachable!(),
                };
                if !pipeline_tokens.is_empty() {
                    result.push((
                        Item::Pipeline(build_pipeline(&pipeline_tokens)?, negated),
                        Some(connector),
                    ));
                    pipeline_tokens.clear();
                    negated = false;
                } else if let Some(last) = result.last_mut() {
                    last.1 = Some(connector);
                } else if connector != Connector::Semi {
                    return Err("unexpected operator".into());
                }
                // Skip consecutive semicolons (from blank lines)
                while i + 1 < tokens.len() && tokens[i + 1] == Token::Semi {
                    i += 1;
                }
            }
            Token::DoubleSemi => return Err("unexpected ';;'".into()),
            Token::LParen => return Err("Opened parentheses without closing".into()),
            _ => {
                pipeline_tokens.push(&tokens[i]);
            }
        }
        i += 1;
    }

    if !pipeline_tokens.is_empty() {
        result.push((
            Item::Pipeline(build_pipeline(&pipeline_tokens)?, negated),
            None,
        ));
    }

    Ok((result, &tokens[i..]))
}

/// Expect `)` at the front of the slice, consuming it.
fn expect_rparen(tokens: &[Token]) -> Result<&[Token], String> {
    match tokens.first() {
        Some(Token::RParen) => Ok(&tokens[1..]),
        Some(t) => Err(format!("expected ')', got '{}'", tok_name(t))),
        None => Err("expected ')'".into()),
    }
}

fn expect_word<'a>(tokens: &'a [Token], word: &str) -> Result<&'a [Token], String> {
    match tokens.first() {
        Some(Token::Word(w)) if w == word => Ok(&tokens[1..]),
        Some(t) => Err(format!("expected '{}', got '{}'", word, tok_name(t))),
        None => Err(format!("expected '{}'", word)),
    }
}

/// Parse `if cond; then body; [elif cond; then body;]... [else body;] fi`
/// Assumes the `if` keyword has already been consumed.
fn parse_if(tokens: &[Token]) -> Result<(Item, &[Token]), String> {
    let mut branches = Vec::new();
    let mut rest = tokens;

    // Parse the initial `if` condition and body
    let (cond, r) = parse_command_line(rest, &["then"])?;
    rest = expect_word(r, "then")?;
    let (body, r) = parse_command_line(rest, &["elif", "else", "fi"])?;
    rest = r;
    branches.push((cond, body));

    // Parse any `elif` branches
    while rest.first() == Some(&Token::Word("elif".into())) {
        rest = &rest[1..];
        let (cond, r) = parse_command_line(rest, &["then"])?;
        rest = expect_word(r, "then")?;
        let (body, r) = parse_command_line(rest, &["elif", "else", "fi"])?;
        rest = r;
        branches.push((cond, body));
    }

    // Parse optional `else`
    let else_body = if rest.first() == Some(&Token::Word("else".into())) {
        rest = &rest[1..];
        let (body, r) = parse_command_line(rest, &["fi"])?;
        rest = r;
        Some(body)
    } else {
        None
    };

    rest = expect_word(rest, "fi")?;

    Ok((
        Item::If {
            branches,
            else_body,
        },
        rest,
    ))
}

/// Parse `while cond; do body; done` or `until cond; do body; done`.
fn parse_while_until(tokens: &[Token], is_until: bool) -> Result<(Item, &[Token]), String> {
    let (condition, rest) = parse_command_line(tokens, &["do"])?;
    let rest = expect_word(rest, "do")?;
    let (body, rest) = parse_command_line(rest, &["done"])?;
    let rest = expect_word(rest, "done")?;
    let item = if is_until {
        Item::Until { condition, body }
    } else {
        Item::While { condition, body }
    };
    Ok((item, rest))
}

/// Parse `for var [in word...]; do body; done`.
fn parse_for(tokens: &[Token]) -> Result<(Item, &[Token]), String> {
    // Expect variable name
    let var = match tokens.first() {
        Some(Token::Word(w)) => w.clone(),
        Some(t) => {
            return Err(format!(
                "expected variable name after 'for', got '{}'",
                tok_name(t)
            ));
        }
        None => return Err("expected variable name after 'for'".into()),
    };
    let mut rest = &tokens[1..];

    // Optional `in word...` — terminated by `;` or `do`
    let mut words = Vec::new();
    if rest.first() == Some(&Token::Word("in".into())) {
        rest = &rest[1..];
        while !rest.is_empty() {
            // Stop at `;` or `do`
            match rest.first() {
                Some(Token::Semi) => {
                    rest = &rest[1..];
                    break;
                }
                Some(Token::Word(w)) if w == "do" => break,
                Some(tok) if is_word_token(tok) => {
                    words.push(tok_to_word(tok));
                    rest = &rest[1..];
                }
                _ => break,
            }
        }
    } else if rest.first() == Some(&Token::Semi) {
        rest = &rest[1..];
    }

    let rest = expect_word(rest, "do")?;
    let (body, rest) = parse_command_line(rest, &["done"])?;
    let rest = expect_word(rest, "done")?;

    Ok((Item::For { var, words, body }, rest))
}

/// Parse `case word in [pattern [| pattern]...) body ;;]... esac`.
fn parse_case(tokens: &[Token]) -> Result<(Item, &[Token]), String> {
    // Expect the word to match on
    let word = match tokens.first() {
        Some(tok) if is_word_token(tok) => tok_to_word(tok),
        Some(t) => return Err(format!("expected word after 'case', got '{}'", tok_name(t))),
        None => return Err("expected word after 'case'".into()),
    };
    let mut rest = &tokens[1..];
    rest = expect_word(rest, "in")?;
    // Skip optional ;
    if rest.first() == Some(&Token::Semi) {
        rest = &rest[1..];
    }

    let mut arms = Vec::new();
    while rest.first() != Some(&Token::Word("esac".into())) {
        if rest.is_empty() {
            return Err("expected 'esac'".into());
        }
        // Optional leading (
        if rest.first() == Some(&Token::LParen) {
            rest = &rest[1..];
        }
        // Parse patterns separated by |
        let mut patterns = Vec::new();
        loop {
            match rest.first() {
                Some(tok) if is_word_token(tok) => {
                    patterns.push(tok_to_word(tok));
                    rest = &rest[1..];
                }
                _ => return Err("expected pattern in case".into()),
            }
            if rest.first() == Some(&Token::Pipe) {
                rest = &rest[1..];
            } else {
                break;
            }
        }
        // Expect )
        rest = expect_rparen(rest)?;
        // Parse body, stopping at ;; or esac
        let (body, r) = parse_command_line(rest, &[";;", "esac"])?;
        rest = r;
        arms.push(CaseArm { patterns, body });
        // Consume ;; if present
        if rest.first() == Some(&Token::DoubleSemi) {
            rest = &rest[1..];
        }
    }
    rest = expect_word(rest, "esac")?;
    Ok((Item::Case { word, arms }, rest))
}

fn is_redirect_token(tok: &Token) -> bool {
    matches!(
        tok,
        Token::RedirectOut
            | Token::RedirectAppend
            | Token::RedirectIn
            | Token::RedirectClobber
            | Token::RedirectReadWrite
            | Token::DupOut
            | Token::DupIn
            | Token::HereDoc
            | Token::HereDocStrip
    )
}

fn build_pipeline(tokens: &[&Token]) -> Result<Pipeline, String> {
    let mut pipeline = Vec::new();
    let mut env_prefix: Vec<(Word, Word)> = Vec::new();
    let mut args: Vec<Word> = Vec::new();
    let mut redirects: Vec<Redirect> = Vec::new();

    let mut i = 0;
    while i < tokens.len() {
        // Check for fd prefix: single digit word followed by redirect token
        let fd_prefix: Option<u32> = if let Token::Word(w) = tokens[i] {
            if w.len() == 1
                && w.as_bytes()[0].is_ascii_digit()
                && i + 1 < tokens.len()
                && is_redirect_token(tokens[i + 1])
            {
                let fd = (w.as_bytes()[0] - b'0') as u32;
                i += 1; // skip the digit, fall through to redirect handling
                Some(fd)
            } else {
                None
            }
        } else {
            None
        };

        match tokens[i] {
            Token::Pipe => {
                if args.is_empty() {
                    return Err("expected command before '|'".into());
                }
                pipeline.push(Command {
                    env: std::mem::take(&mut env_prefix),
                    args: std::mem::take(&mut args),
                    redirects: std::mem::take(&mut redirects),
                });
            }
            Token::RedirectOut
            | Token::RedirectAppend
            | Token::RedirectClobber
            | Token::RedirectIn
            | Token::RedirectReadWrite
            | Token::DupOut
            | Token::DupIn
            | Token::HereDoc
            | Token::HereDocStrip => {
                let tok = tokens[i].clone();
                i += 1;
                if i >= tokens.len() || !is_word_token(tokens[i]) {
                    return Err(format!("expected filename after '{}'", tok_name(&tok)));
                }
                let word = tok_to_word(tokens[i]);
                let redir = match tok {
                    Token::RedirectOut => Redirect::Write(fd_prefix.unwrap_or(1), word),
                    Token::RedirectAppend => Redirect::Append(fd_prefix.unwrap_or(1), word),
                    Token::RedirectClobber => Redirect::Clobber(fd_prefix.unwrap_or(1), word),
                    Token::RedirectIn => Redirect::Read(fd_prefix.unwrap_or(0), word),
                    Token::RedirectReadWrite => Redirect::ReadWrite(fd_prefix.unwrap_or(0), word),
                    Token::DupOut => Redirect::DupWrite(fd_prefix.unwrap_or(1), word),
                    Token::DupIn => Redirect::DupRead(fd_prefix.unwrap_or(0), word),
                    Token::HereDoc => {
                        let delim = word_to_str(&word).unwrap_or_default();
                        let quoted = word.iter().any(|p| {
                            matches!(p, WordPart::SingleQuoted(_) | WordPart::DoubleQuoted(_))
                        });
                        Redirect::HereDoc(
                            fd_prefix.unwrap_or(0),
                            delim,
                            String::new(),
                            false,
                            quoted,
                        )
                    }
                    Token::HereDocStrip => {
                        let delim = word_to_str(&word).unwrap_or_default();
                        let quoted = word.iter().any(|p| {
                            matches!(p, WordPart::SingleQuoted(_) | WordPart::DoubleQuoted(_))
                        });
                        Redirect::HereDoc(
                            fd_prefix.unwrap_or(0),
                            delim,
                            String::new(),
                            true,
                            quoted,
                        )
                    }
                    _ => unreachable!(),
                };
                redirects.push(redir);
            }
            Token::Word(_) | Token::WordParts(_) => {
                let word = tok_to_word(tokens[i]);
                // KEY=VALUE before the command name is an env prefix
                if args.is_empty() {
                    match tokens[i] {
                        Token::Word(w) => {
                            if let Some(eq) = w.find('=') {
                                env_prefix.push((
                                    vec![WordPart::Literal(w[..eq].to_string())],
                                    vec![WordPart::Literal(w[eq + 1..].to_string())],
                                ));
                                i += 1;
                                continue;
                            }
                        }
                        Token::WordParts(parts) => {
                            if let Some(WordPart::Literal(s)) = parts.first()
                                && let Some(eq) = s.find('=')
                            {
                                let key = vec![WordPart::Literal(s[..eq].to_string())];
                                let mut val: Word =
                                    vec![WordPart::Literal(s[eq + 1..].to_string())];
                                val.extend_from_slice(&parts[1..]);
                                env_prefix.push((key, val));
                                i += 1;
                                continue;
                            }
                        }
                        _ => {}
                    }
                }
                args.push(word);
            }
            _ => return Err(format!("unexpected token: {}", tok_name(tokens[i]))),
        }
        i += 1;
    }

    if args.is_empty() && env_prefix.is_empty() && redirects.is_empty() {
        return Err("expected command".into());
    }
    pipeline.push(Command {
        env: env_prefix,
        args,
        redirects,
    });

    Ok(pipeline)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(input: &str) -> Result<CommandLine, String> {
        parse(input)
    }

    /// Helper: extract the literal string from a Word (panics if not all literal/single-quoted).
    fn word_str(w: &Word) -> String {
        word_to_str(w).expect("expected literal word")
    }

    /// Helper: extract string args from a pipeline command.
    fn cmd_args(cmd: &Command) -> Vec<String> {
        cmd.args.iter().map(word_str).collect()
    }

    fn pipelines(cl: &CommandLine) -> Vec<(&[Command], Option<Connector>)> {
        cl.iter()
            .map(|(item, c)| match item {
                Item::Pipeline(p, _) => (p.as_slice(), c.clone()),
                _ => panic!("expected pipeline"),
            })
            .collect()
    }

    #[test]
    fn simple_command() {
        let result = p("ls -la /tmp").unwrap();
        let p = pipelines(&result);
        assert_eq!(p.len(), 1);
        assert_eq!(cmd_args(&p[0].0[0]), vec!["ls", "-la", "/tmp"]);
        assert_eq!(p[0].1, None);
    }

    #[test]
    fn pipeline() {
        let result = p("cat foo | grep bar").unwrap();
        let p = pipelines(&result);
        assert_eq!(p[0].0.len(), 2);
        assert_eq!(cmd_args(&p[0].0[0]), vec!["cat", "foo"]);
        assert_eq!(cmd_args(&p[0].0[1]), vec!["grep", "bar"]);
    }

    #[test]
    fn semicolons() {
        let result = p("echo a; echo b").unwrap();
        let p = pipelines(&result);
        assert_eq!(p.len(), 2);
        assert_eq!(cmd_args(&p[0].0[0]), vec!["echo", "a"]);
        assert_eq!(p[0].1, Some(Connector::Semi));
        assert_eq!(cmd_args(&p[1].0[0]), vec!["echo", "b"]);
        assert_eq!(p[1].1, None);
    }

    #[test]
    fn and_chain() {
        let result = p("true && echo yes").unwrap();
        let p = pipelines(&result);
        assert_eq!(p.len(), 2);
        assert_eq!(cmd_args(&p[0].0[0]), vec!["true"]);
        assert_eq!(p[0].1, Some(Connector::And));
    }

    #[test]
    fn or_chain() {
        let result = p("false || echo fallback").unwrap();
        let p = pipelines(&result);
        assert_eq!(p.len(), 2);
        assert_eq!(cmd_args(&p[0].0[0]), vec!["false"]);
        assert_eq!(p[0].1, Some(Connector::Or));
    }

    #[test]
    fn mixed_connectors() {
        let result = p("cmd1 && cmd2 || cmd3; cmd4").unwrap();
        let p = pipelines(&result);
        assert_eq!(p.len(), 4);
        assert_eq!(p[0].1, Some(Connector::And));
        assert_eq!(p[1].1, Some(Connector::Or));
        assert_eq!(p[2].1, Some(Connector::Semi));
        assert_eq!(p[3].1, None);
    }

    #[test]
    fn redirect_out() {
        let result = p("echo hello > out.txt").unwrap();
        let p = pipelines(&result);
        assert!(
            matches!(&p[0].0[0].redirects[0], Redirect::Write(1, w) if word_str(w) == "out.txt")
        );
    }

    #[test]
    fn redirect_append() {
        let result = p("echo hello >> out.txt").unwrap();
        let p = pipelines(&result);
        assert!(
            matches!(&p[0].0[0].redirects[0], Redirect::Append(1, w) if word_str(w) == "out.txt")
        );
    }

    #[test]
    fn redirect_in() {
        let result = p("cat < in.txt").unwrap();
        let p = pipelines(&result);
        assert!(matches!(&p[0].0[0].redirects[0], Redirect::Read(0, w) if word_str(w) == "in.txt"));
    }

    #[test]
    fn single_quotes() {
        let result = p("echo 'hello world'").unwrap();
        let p = pipelines(&result);
        assert_eq!(cmd_args(&p[0].0[0]), vec!["echo", "hello world"]);
    }

    #[test]
    fn double_quotes() {
        let result = p(r#"echo "hello world""#).unwrap();
        let p = pipelines(&result);
        // Double-quoted literal becomes DoubleQuoted([Literal("hello world")])
        assert_eq!(p[0].0[0].args.len(), 2);
    }

    #[test]
    fn escaped_in_double_quotes() {
        let result = p(r#"echo "hello \"world\"""#).unwrap();
        let p = pipelines(&result);
        assert_eq!(p[0].0[0].args.len(), 2);
    }

    #[test]
    fn empty_input() {
        assert!(p("").unwrap().is_empty());
    }

    #[test]
    fn unterminated_quote() {
        assert!(p("echo 'hello").is_err());
    }

    #[test]
    fn bare_ampersand() {
        let result = p("echo hello &").unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].1, Some(Connector::Background));
    }

    #[test]
    fn simple_group() {
        let result = p("true && (echo a; echo b)").unwrap();
        assert_eq!(result.len(), 2);
        assert!(matches!(&result[0].0, Item::Pipeline(_, _)));
        assert_eq!(result[0].1, Some(Connector::And));
        match &result[1].0 {
            Item::Subshell(cl) => {
                let p = pipelines(cl);
                assert_eq!(p.len(), 2);
                assert_eq!(cmd_args(&p[0].0[0]), vec!["echo", "a"]);
                assert_eq!(cmd_args(&p[1].0[0]), vec!["echo", "b"]);
            }
            _ => panic!("expected subshell"),
        }
    }

    #[test]
    fn group_with_connector_after() {
        let result = p("(false || true) && echo ok").unwrap();
        assert_eq!(result.len(), 2);
        assert!(matches!(&result[0].0, Item::Subshell(_)));
        assert_eq!(result[0].1, Some(Connector::And));
        match &result[1].0 {
            Item::Pipeline(p, _) => assert_eq!(cmd_args(&p[0]), vec!["echo", "ok"]),
            _ => panic!("expected pipeline"),
        }
    }

    #[test]
    fn unterminated_group() {
        assert!(p("(echo hello").is_err());
    }

    #[test]
    fn unexpected_rparen() {
        assert!(p("echo hello)").is_err());
    }

    #[test]
    fn empty_group() {
        assert!(p("()").is_err());
    }

    #[test]
    fn if_then_fi() {
        let result = p("if true; then echo hello; fi").unwrap();
        assert_eq!(result.len(), 1);
        match &result[0].0 {
            Item::If {
                branches,
                else_body,
            } => {
                assert_eq!(branches.len(), 1);
                let cond = pipelines(&branches[0].0);
                assert_eq!(cmd_args(&cond[0].0[0]), vec!["true"]);
                let body = pipelines(&branches[0].1);
                assert_eq!(cmd_args(&body[0].0[0]), vec!["echo", "hello"]);
                assert!(else_body.is_none());
            }
            _ => panic!("expected if"),
        }
    }

    #[test]
    fn if_else() {
        let result = p("if false; then echo yes; else echo no; fi").unwrap();
        match &result[0].0 {
            Item::If {
                branches,
                else_body,
            } => {
                assert_eq!(branches.len(), 1);
                let body = pipelines(&branches[0].1);
                assert_eq!(cmd_args(&body[0].0[0]), vec!["echo", "yes"]);
                let eb = pipelines(else_body.as_ref().unwrap());
                assert_eq!(cmd_args(&eb[0].0[0]), vec!["echo", "no"]);
            }
            _ => panic!("expected if"),
        }
    }

    #[test]
    fn if_elif_else() {
        let result = p("if false; then echo a; elif true; then echo b; else echo c; fi").unwrap();
        match &result[0].0 {
            Item::If {
                branches,
                else_body,
            } => {
                assert_eq!(branches.len(), 2);
                let b0 = pipelines(&branches[0].1);
                assert_eq!(cmd_args(&b0[0].0[0]), vec!["echo", "a"]);
                let b1 = pipelines(&branches[1].1);
                assert_eq!(cmd_args(&b1[0].0[0]), vec!["echo", "b"]);
                let eb = pipelines(else_body.as_ref().unwrap());
                assert_eq!(cmd_args(&eb[0].0[0]), vec!["echo", "c"]);
            }
            _ => panic!("expected if"),
        }
    }

    #[test]
    fn if_with_connector() {
        let result = p("if true; then echo yes; fi && echo after").unwrap();
        assert_eq!(result.len(), 2);
        assert!(matches!(&result[0].0, Item::If { .. }));
        assert_eq!(result[0].1, Some(Connector::And));
    }

    #[test]
    fn unterminated_if() {
        assert!(p("if true; then echo hello").is_err());
    }

    #[test]
    fn if_missing_then() {
        assert!(p("if true; echo hello; fi").is_err());
    }

    #[test]
    fn var_expansion_preserved() {
        // Parser should preserve $FOO as a Var node, not expand it
        let result = p("echo $FOO").unwrap();
        let p = pipelines(&result);
        assert_eq!(p[0].0[0].args.len(), 2);
        assert!(matches!(&p[0].0[0].args[1][0], WordPart::Var(name) if name == "FOO"));
    }

    #[test]
    fn var_expansion_braces_preserved() {
        let result = p("echo ${X}world").unwrap();
        let p = pipelines(&result);
        assert_eq!(p[0].0[0].args.len(), 2);
        assert!(matches!(&p[0].0[0].args[1][0], WordPart::Var(name) if name == "X"));
        assert!(matches!(&p[0].0[0].args[1][1], WordPart::Literal(s) if s == "world"));
    }

    #[test]
    fn single_quotes_no_expansion() {
        let result = p("echo '$FOO'").unwrap();
        let p = pipelines(&result);
        assert_eq!(cmd_args(&p[0].0[0]), vec!["echo", "$FOO"]);
    }

    #[test]
    fn env_prefix() {
        let result = p("FOO=bar BAZ=qux echo hello").unwrap();
        match &result[0].0 {
            Item::Pipeline(p, _) => {
                assert_eq!(p[0].env.len(), 2);
                assert_eq!(word_str(&p[0].env[0].0), "FOO");
                assert_eq!(word_str(&p[0].env[0].1), "bar");
                assert_eq!(word_str(&p[0].env[1].0), "BAZ");
                assert_eq!(word_str(&p[0].env[1].1), "qux");
                assert_eq!(cmd_args(&p[0]), vec!["echo", "hello"]);
            }
            _ => panic!("expected pipeline"),
        }
    }

    #[test]
    fn backtick_preserved() {
        let result = p("echo `ls`").unwrap();
        let p = pipelines(&result);
        assert!(matches!(&p[0].0[0].args[1][0], WordPart::Backtick(cmd) if cmd == "ls"));
    }

    #[test]
    fn double_quote_var_preserved() {
        let result = p(r#"echo "$FOO world""#).unwrap();
        let p = pipelines(&result);
        match &p[0].0[0].args[1][0] {
            WordPart::DoubleQuoted(parts) => {
                assert!(matches!(&parts[0], WordPart::Var(name) if name == "FOO"));
                assert!(matches!(&parts[1], WordPart::Literal(s) if s == " world"));
            }
            _ => panic!("expected double-quoted"),
        }
    }

    #[test]
    fn unexpected_token_digit_paren() {
        assert!(p("1(").is_err());
        assert!(p("0(").is_err());
        assert!(p("2(").is_err());
        assert!(p("99(").is_err());
    }

    #[test]
    fn unexpected_token_double_semi() {
        assert!(p(";;").is_err());
    }
}
