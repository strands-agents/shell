printf 'hello\n' > a
printf '\377\000\200' > bin
md5sum a > sums
md5sum -b bin >> sums
md5sum -c sums
echo "status=$?"
md5sum -c --quiet sums
echo "status=$?"
md5sum -c - < sums
echo "status=$?"
printf 'changed\n' > a
md5sum -c sums 2>/dev/null
echo "status=$?"
md5sum -c --quiet sums 2>/dev/null
echo "status=$?"
md5sum -c --status sums
echo "status=$?"
rm bin
md5sum -c sums 2>/dev/null
echo "status=$?"
