printf 'hello\n' > a
sha256sum a > sums
printf 'not a checksum line\n# a comment\n' >> sums
sha256sum -c sums
echo "status=$?"
printf 'garbage\n' > bad
sha256sum -c bad
echo "status=$?"
