# known-difference: cat doesn't treat - as stdin
echo one > a.txt
echo two > b.txt
cat a.txt b.txt
cat a.txt - b.txt < /dev/null
