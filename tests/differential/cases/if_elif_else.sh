for n in 1 5 10; do
  if [ "$n" -lt 3 ]; then echo "$n small"
  elif [ "$n" -lt 7 ]; then echo "$n medium"
  else echo "$n large"
  fi
done
