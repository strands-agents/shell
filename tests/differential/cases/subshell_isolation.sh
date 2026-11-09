x=1
(x=2; echo "inner=$x")
echo "outer=$x"
{ y=3; }
echo "group=$y"
