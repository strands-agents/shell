# A few thousand lines read one by one from a redirected file: each read
# takes exactly one line and leaves the rest of the file unread.
i=0
while [ $i -lt 3000 ]; do
  i=$((i+1))
  if [ $((i % 500)) -eq 0 ]; then echo; else echo "line $i"; fi
done > data.txt
n=0; empty=0; last=
while IFS= read -r l; do
  n=$((n+1))
  [ -z "$l" ] && empty=$((empty+1))
  last=$l
done < data.txt
echo "lines=$n empty=$empty last=$last"
{ read -r a; read -r b; head -n 2; } < data.txt
{ i=0; while [ $i -lt 2997 ]; do read -r x; i=$((i+1)); done; cat; } < data.txt
