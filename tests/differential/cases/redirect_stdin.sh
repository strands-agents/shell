printf 'x\ny\n' > in.txt
read first < in.txt
echo "first=$first"
while read line; do echo "got $line"; done < in.txt
