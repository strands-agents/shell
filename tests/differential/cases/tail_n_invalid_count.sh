# tail rejects a -n count it can't parse instead of using a default (#253).
printf '1\n2\n3\n' > f.txt
tail -n abc f.txt
echo "rc=$?"
tail -n +abc f.txt
echo "rc=$?"
tail -n -x f.txt
echo "rc=$?"
tail -n '' f.txt
