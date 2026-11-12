# known-difference: &> isn't supported, so the output isn't redirected
cat missing-file &> all.txt
echo "status=$?"
[ -s all.txt ] && echo "all has content"
