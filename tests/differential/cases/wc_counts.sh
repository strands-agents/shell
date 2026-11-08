# known-difference: wc pads a single count read from stdin to 7 columns; GNU prints the bare number
printf 'a b\nc d e\n' > f.txt
wc -l < f.txt
wc -w < f.txt
wc -c < f.txt
