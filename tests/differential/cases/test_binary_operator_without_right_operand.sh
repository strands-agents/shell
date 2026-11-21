[ x = ]; echo "rc=$?"
h=/home/user; e=; [ "$h" = $e ]; echo "rc=$?"
test x -eq; echo "rc=$?"
[ x y ]; echo "rc=$?"
[ ! x = ]; echo "rc=$?"
[ ! x -a ]; echo "rc=$?"
x=; [ ! $x -lt 3 ]; echo "rc=$?"
echo after
