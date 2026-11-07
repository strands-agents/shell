# known-difference: tail doesn't support -c
printf 'abcdef' > f.txt
head -c 3 f.txt
echo
tail -c 2 f.txt
echo
