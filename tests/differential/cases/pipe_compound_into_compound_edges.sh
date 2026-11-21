{ cat; } <<A | while read x; do echo "w:$x"; done
one
A
{ echo a; } | while read x; do echo "r:$x"; done <<B
two
B
{ cat <<C; } | { cat; cat <<D; }
three
C
four
D
set -- p q; { echo a; } | { cat; echo "$@"; }
y=0; { echo a; } | while read x; do y=1; done; echo "y=$y"
set -o pipefail; { false; } | { cat; }; echo "pf:$?"; set +o pipefail
f() { { echo in; } | while read l; do echo "f:$l"; done; }; f
{ echo a; } | [[ -n x ]] && echo cond
{ echo a; } | while read x
do
  echo "ml:$x"
done
if true; then { echo a; } | { cat; }; fi
( echo a ) | if true; then cat; fi > /dev/null; echo "redir:$?"
