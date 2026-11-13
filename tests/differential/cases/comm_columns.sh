printf 'apple\nbanana\ndate\n' > one
printf 'banana\ncherry\ndate\nfig\n' > two
comm one two
comm -1 one two
comm -2 one two
comm -3 one two
comm -12 one two
comm -23 one two
comm --output-delimiter=' | ' one two
comm - two < one
printf 'b\na\n' > unsorted
comm --nocheck-order unsorted two
echo "status $?"
comm unsorted two
echo "status $?"
comm --check-order one unsorted
echo "status $?"
