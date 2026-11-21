# cat reports each operand it can't open and still prints the rest (#335).
echo body > f
mkdir d
cat missing f d f
echo "rc=$?"
