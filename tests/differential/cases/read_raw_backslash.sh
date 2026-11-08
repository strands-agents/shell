# known-difference: read without -r keeps backslashes; bash removes them
printf 'a\\tb\n' > raw.txt
read -r line < raw.txt
printf '%s\n' "$line"
read line < raw.txt
printf '%s\n' "$line"
