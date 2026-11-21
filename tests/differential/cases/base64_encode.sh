printf 'hello' | base64
printf '\377\376\000\001\200' > bin
base64 bin
base64 - < bin
printf '' | base64
head -c 60 /dev/zero | base64
head -c 57 /dev/zero | base64
printf 'abcdefghijklmnopqrstuvwxyz' | base64 -w 10
printf 'abcdefghijklmnopqrstuvwxyz' | base64 --wrap=0
echo
