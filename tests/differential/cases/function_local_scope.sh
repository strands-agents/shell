x=global
f() { local x=local; echo "in $x"; }
f
echo "out $x"
