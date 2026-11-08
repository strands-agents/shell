# known-difference: fixed by #205
echo 'k1=v1' | { IFS== read key value; echo "$key -> $value"; }
