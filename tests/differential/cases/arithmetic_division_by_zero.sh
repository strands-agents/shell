# known-difference: $((1 / 0)) prints 0 with no error; bash reports division by zero
# compare: stdout, status-class, stderr-presence
echo $((1 / 0))
echo after
