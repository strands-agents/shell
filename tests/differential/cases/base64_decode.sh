printf 'aGVsbG8=\n' | base64 -d
echo
printf 'aGVs\nbG8=' | base64 --decode
echo
printf 'aG==aGk=' | base64 -d
echo
printf 'a*G*V*s\r\nbG8=' | base64 -d -i
echo
printf '\377\376\000\001\200' | base64 | base64 -d | base64
