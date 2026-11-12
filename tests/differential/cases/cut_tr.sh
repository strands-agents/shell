printf 'a,b,c\n1,2,3\n' | cut -d, -f2
printf 'a,b,c\n' | cut -d, -f1,3
echo hello | tr a-z A-Z
echo 'a  b   c' | tr -s ' '
echo 'h-e-y' | tr -d -
