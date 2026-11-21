printf 'a\nb\n' | while read x; do echo "while:$x"; done
printf 'a\nb\n' | for w in one two; do read x; echo "for:$w:$x"; done
printf 'a\nb\n' | if read x; then echo "if:$x"; else echo "if:none"; fi
printf 'a\nb\n' | case z in z) read x; echo "case:$x" ;; esac
printf 'a\nb\n' | { read x; echo "group:$x"; }
printf 'a\nb\n' | ( read x; echo "subshell:$x" )
