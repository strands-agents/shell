printf '1\n2\n3\n' > nums
printf 'a\nb\n' > letters
paste nums letters
paste -d, nums letters nums
paste -d ',;' nums letters nums letters
paste -s nums letters
paste -s -d ',;' nums
paste -d '\n' nums letters
printf 'x\ny\nz\nw\n' | paste - -
paste nums - letters - < letters
printf 'a\nb' | paste -s
paste nums missing
echo "status $?"
