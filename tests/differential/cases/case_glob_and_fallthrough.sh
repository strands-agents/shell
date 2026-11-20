for v in yes Y no maybe "" ; do
  case "$v" in
    [Yy]|[Yy]es) echo "[$v] yes" ;;
    n*) echo "[$v] no" ;;
    "") echo "[$v] empty" ;;
    *) echo "[$v] unknown" ;;
  esac
done
