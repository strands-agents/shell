printf 'hello\n' > a
md5sum a > sums
printf 'changed\n' > a
md5sum -c --status --quiet sums 2>/dev/null
echo "status=$?"
md5sum -c --quiet --status sums
echo "status=$?"
