cat missing-file 2>&1 >/dev/null | grep -q . && echo "stderr went to pipe"
