printf 'hello\n' > a
sha256sum a > sums
printf 'changed\n' > a
sha256sum -c --status --quiet sums 2>/dev/null
echo "status=$?"
sha256sum -c --quiet --status sums
echo "status=$?"
