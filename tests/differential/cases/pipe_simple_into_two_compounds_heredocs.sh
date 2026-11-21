echo x | { cat; cat <<B; } | { cat; cat <<C; }
two
B
three
C
echo after-1
{ echo zero; } | cat | { cat; cat <<B; } | { cat; cat <<C; }
two
B
three
C
echo after-2
