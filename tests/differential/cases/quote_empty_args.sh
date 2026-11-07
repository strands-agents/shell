# known-difference: f "" "" passes no arguments; bash passes two empty ones
f() { echo "$#"; }
f "" ""
f ''
e=
f $e
f "$e"
