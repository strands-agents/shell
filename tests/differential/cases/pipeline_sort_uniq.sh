printf 'b\na\nb\nc\na\nb\n' | sort | uniq -c
printf 'b\na\nb\n' | sort -u | head -n 1
