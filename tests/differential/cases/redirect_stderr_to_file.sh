cat missing-file 2> err.txt
echo "status=$?"
[ -s err.txt ] && echo "err has content"
