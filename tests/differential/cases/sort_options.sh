printf '10\n9\n100\n9\n' > nums.txt
sort -n nums.txt
sort -rn nums.txt
sort -u nums.txt
printf 'b 2\na 3\nc 1\n' | sort -k2 -n
printf 'x:3\ny:1\n' | sort -t: -k2
