echo 'k1=v1' | { IFS== read key value; echo "$key -> $value"; }
