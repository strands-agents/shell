readonly r
echo "r=[${r-unset}]"
r=1
echo "after assignment r=[${r-unset}]"
readonly r2=1
export r2
env | grep '^r2='
r2=2
echo "r2=$r2"
x=1 env | grep '^x='
echo "after prefix x=[${x-unset}]"
export a=1 b=2
env | grep -E '^(a|b)=' | sort
a=changed
env | grep '^a='
c=3
export c
env | grep '^c='
export y=2
unset y
env | grep '^y=' || echo "y gone from env"
echo "y=[${y-unset}]"
y=3
echo "y=$y"
export z=1
export -n z
echo "z=$z"
export w=outer
f() { local w=inner; env | grep '^w='; }
f
env | grep '^w='
echo "w=$w"
v=global
g() { local v; v=set; echo "in g v=$v"; }
g
echo "after g v=$v"
h() { local q=1; }
h
echo "q=[${q-unset}]"
