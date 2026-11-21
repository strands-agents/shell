set -e
f() { false; echo in-f; }
! f
! { false; echo in-g; }
! ( false; echo in-s )
! eval 'false; echo in-e'
echo after
