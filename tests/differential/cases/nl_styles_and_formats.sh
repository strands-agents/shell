printf 'alpha\n\nbeta\n \ngamma' > f
nl f
nl -b a f
nl -b n f
nl -n ln f
nl -n rz -w 3 -s ': ' f
nl -ba -nrz -w2 -s. f - f < f
printf '%s\n' a b c d e f g h i j k | nl -w 1 | tail -2
nl -b x f
echo "status $?"
