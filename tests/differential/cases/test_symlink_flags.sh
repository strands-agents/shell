mkdir d
echo x > f
ln -s d ld
ln -s f lf
ln -s missing dangling
test -L ld; echo "test -L ld: $?"
[ -h ld ]; echo "[ -h ld ]: $?"
test -L lf; echo "test -L lf: $?"
[ -h dangling ]; echo "[ -h dangling ]: $?"
test -L d; echo "test -L d: $?"
[ -h f ]; echo "[ -h f ]: $?"
[ -h missing ]; echo "[ -h missing ]: $?"
