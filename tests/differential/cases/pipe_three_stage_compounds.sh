{ echo a; echo b; } | ( cat; echo c ) | while read x; do echo "[$x]"; done
( echo a ) | { cat; echo b; } | ( cat; echo c )
{ echo a; } | cat | while read x; do echo "w:$x"; done
echo a | { cat; echo b; } | while read x; do echo "s:$x"; done
{ echo a; echo b; } | while read x; do echo "$x$x"; done | sort -r
{ echo z; } | if true; then cat; fi | { cat; echo end; }
! { echo a; } | { cat >/dev/null; false; }; echo "negated:$?"
{ echo a; } | { cat; exit 3; }; echo "status:$?"
