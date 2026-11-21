echo -e 'A\0303\0251B'
echo -e 'A\0303\0251B' | wc -c | tr -d ' '
echo -e '\0377\xff\200' | wc -c | tr -d ' '
echo -e 'A\0101\01012B'
