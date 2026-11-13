# known-difference: a function named after a builtin never runs, and command -V reports the builtin (#174)
echo() { builtin echo fn; }
command -V echo
echo hi
