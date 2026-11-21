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
