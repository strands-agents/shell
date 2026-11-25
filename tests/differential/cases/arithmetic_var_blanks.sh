n=" 5"
echo $((n + 1))
n="5 "
echo $((n + 1))
n="	-5
"
echo $((n + 1))
n="      2"
echo $((n + 1))
n=" 5 "
echo $((++n))
echo "$n"
n=" 5 "
: $((n += 2))
echo "$n"
