for i in 1 2 3 4 5; do
  [ "$i" -eq 2 ] && continue
  [ "$i" -eq 4 ] && break
  echo "$i"
done
