[ x = ]; echo "rc=$?"
e=; [ "$HOME" = $e ]; echo "rc=$?"
test x -eq; echo "rc=$?"
[ x y ]; echo "rc=$?"
[ ! x = ]; echo "rc=$?"
echo after
