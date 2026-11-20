f() { echo "$#"; }
f "" ""
f ''
e=
f $e
f "$e"
unset u
f "$u"
set --
f "$@"
f "$@"""
f "$@$u"
