set -u
i=0
while [ $i -lt 200 ]; do
  r=$RANDOM
  if [ "$r" -lt 0 ] || [ "$r" -gt 32767 ]; then
    echo "out of range: $r"
  fi
  i=$((i + 1))
done
echo done
