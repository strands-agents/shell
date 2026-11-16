printf 'hello\n' > a
printf '\377\000\200' > bin
sha256sum a > sums
sha256sum -b bin >> sums
sha256sum -c sums
echo "status=$?"
sha256sum -c --quiet sums
echo "status=$?"
sha256sum -c - < sums
echo "status=$?"
printf 'changed\n' > a
sha256sum -c sums 2>/dev/null
echo "status=$?"
sha256sum -c --quiet sums 2>/dev/null
echo "status=$?"
sha256sum -c --status sums
echo "status=$?"
rm bin
sha256sum -c sums 2>/dev/null
echo "status=$?"
