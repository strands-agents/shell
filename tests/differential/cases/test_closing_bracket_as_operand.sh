# Only `[` takes a closing `]`; `test` reads `]` as an ordinary word.
t() { err=$("$@" 2>&1); echo "rc=$? ${err:+diag}"; }
t test x = ']'
t test ']' = ']'
t test ']'
t test x ']'
t test ! ']'
t test -n ']'
t [ x = ] ]
t [ ] = ] ]
t [ x = ]
t [ x
t [ ]
t [
