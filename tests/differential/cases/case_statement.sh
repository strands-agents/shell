for f in a.txt b.log c; do
  case "$f" in
    *.txt) echo "$f text" ;;
    *.log|*.out) echo "$f log" ;;
    *) echo "$f other" ;;
  esac
done
