set -e
! { echo a; } | { cat; }; echo "after:$?"
! { echo ERROR; } | { grep -q ERROR; }; echo "after:$?"
! ( echo b ) | while read x; do echo "w:$x"; done; echo "after:$?"
