# Quoted parts of a case pattern match literally; unquoted parts stay glob.
case abc in "a*") echo matched ;; *) echo nomatch ;; esac
case abc in 'a*') echo matched ;; *) echo nomatch ;; esac
x="*"; case abc in "$x") echo matched ;; *) echo nomatch ;; esac
case abc in $x) echo unquoted-var-globs ;; *) echo nomatch ;; esac
f="notes[1].txt"; case "$f" in "$f") echo same ;; *) echo differ ;; esac
case "a*" in a"*") echo literal-star ;; *) echo nomatch ;; esac
case abc in a"*") echo matched ;; *) echo mixed-nomatch ;; esac
case "a?c" in "a?"*) echo mixed-prefix ;; *) echo nomatch ;; esac
case 'a\b' in 'a\b') echo backslash ;; *) echo nomatch ;; esac
# The same patterns when the case runs inside a command substitution.
out=$(case abc in ("a*") echo matched ;; (*) echo nomatch ;; esac); echo "sub: $out"
out=$(case "$f" in ("$f") echo same ;; (*) echo differ ;; esac); echo "sub: $out"
# An unquoted escaped backslash still matches a backslash.
p='C:\Users\me'; case $p in *\\*) echo has-backslash ;; *) echo no-backslash ;; esac
p2='/usr/*'; case $p2 in *\\*) echo has-backslash ;; *) echo no-backslash ;; esac
case 'a\b' in a\\b) echo match ;; *) echo nomatch ;; esac
s='\n'; case $s in \\[nt]) echo escape ;; *) echo plain ;; esac
out=$(case $p in (*\\*) echo has-backslash ;; (*) echo no-backslash ;; esac); echo "sub: $out"
# Quoted glob characters inside a bracket expression are just members of the set.
case '\' in ['*?']) echo glob-char ;; *) echo plain ;; esac
case 'a\b' in *["*?"]*) echo has-glob ;; *) echo plain ;; esac
case 'a*b' in *["*?"]*) echo has-glob ;; *) echo plain ;; esac
case '\' in [!"*"]) echo not-star ;; *) echo star ;; esac
# A backslash in an unquoted variable escapes the next pattern character.
x='a\b'; case ab in $x) echo var-escape ;; *) echo nomatch ;; esac
case 'a\b' in $x) echo matched ;; *) echo var-nomatch ;; esac
# A tilde expands to a literal path.
HOME='/tmp/a\b'; case '/tmp/a\b' in ~) echo tilde ;; *) echo nomatch ;; esac
# A backslash or quoted character can be a range endpoint.
case 'Z' in [A-\\]) echo in-range ;; *) echo out ;; esac
case 'a' in [A-\\]) echo in-range ;; *) echo out ;; esac
case ']' in [\\-_]) echo in-range ;; *) echo out ;; esac
case 'a' in ["*"-z]) echo in-range ;; *) echo out ;; esac
case '!' in ["*"-z]) echo in-range ;; *) echo out ;; esac
case '\' in [\\/]) echo member ;; *) echo out ;; esac
[[ Z == [A-\\] ]] && echo cond-in-range || echo cond-out
