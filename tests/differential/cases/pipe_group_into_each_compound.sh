{ echo a; echo b; } | while read x; do echo "while:$x"; done
{ echo a; echo b; } | for w in one two; do read x; echo "for:$w:$x"; done
{ echo a; echo b; } | if read x; then echo "if:$x"; else echo "if:none"; fi
{ echo a; echo b; } | case z in z) read x; echo "case:$x" ;; esac
{ echo a; echo b; } | { read x; echo "group:$x"; }
{ echo a; echo b; } | ( read x; echo "subshell:$x" )
