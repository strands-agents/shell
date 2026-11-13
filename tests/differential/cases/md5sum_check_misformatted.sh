printf 'hello\n' > a
md5sum a > sums
printf 'not a checksum line\n# a comment\n' >> sums
md5sum -c sums
echo "status=$?"
printf 'garbage\n' > bad
md5sum -c bad
echo "status=$?"
