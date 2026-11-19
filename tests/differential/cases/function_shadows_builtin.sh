# known-difference: a function named after a builtin never runs (#174)
echo() { builtin echo fn; }
echo hi
