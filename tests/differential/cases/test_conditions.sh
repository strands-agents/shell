echo x > file.txt
mkdir dir
[ -f file.txt ] && echo is-file
[ -d dir ] && echo is-dir
[ -e missing ] || echo no-missing
[ -z "" ] && echo empty
[ -n "x" ] && echo non-empty
[ "a" = "a" ] && echo equal
[ 3 -gt 2 ] && echo greater
test 1 -eq 2
echo "status=$?"
