i=0
while [ "$i" -lt 3 ]; do
  i=$((i + 1))
  echo "$i"
done
until [ "$i" -eq 0 ]; do i=$((i - 1)); done
echo "done $i"
