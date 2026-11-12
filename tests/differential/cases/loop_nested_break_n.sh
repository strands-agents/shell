for i in a b; do
  for j in 1 2 3; do
    [ "$j" -eq 2 ] && break 2
    echo "$i$j"
  done
done
echo done
