x=$(echo hello | tr a-z A-Z)
echo "$x"
echo "last=$(printf '%s\n' a b c | tail -n 1)"
echo "nested=$(echo $(echo inner))"
echo "backtick=`echo tick`"
