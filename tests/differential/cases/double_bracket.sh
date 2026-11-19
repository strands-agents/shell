printf 'x\n' > file
[[ -n x ]] && echo nonempty
[[ -z x ]] || echo not-empty
[[ -f file && ! -d file ]] && echo file
[[ -d . || -f nosuch ]] && echo either
[[ ( -f nosuch || -f file ) && -n x ]] && echo grouped
v="a b"
[[ $v == "a b" ]] && echo no-split
[[ foo.rs == *.rs ]] && echo pattern
[[ foo.rs != *.txt ]] && echo not-pattern
[[ *.rs == *.rs ]] && echo no-glob
[[ a < b && b > a ]] && echo order
[[ 3 -lt 12 ]] && echo numeric
os=darwin
if [[ -n ${ZSH_VERSION:-} ]]; then echo zsh
elif [[ "$os" == "msys" ]] || [[ "$os" == "cygwin" ]]; then echo windows
else echo other; fi
[[ -x file ]]
echo "status=$?"
[[ -x $unset_var ]] || echo empty-not-executable
[[ ! -d "" ]] && echo empty-not-dir
[[ "$unset_var" == "msys" ]] || echo empty-operand
x="&&"
[[ $x == "&&" ]] && echo operator-text-is-operand
case x in x) [[ -n a && -n b ]] && echo case-arm;; esac
[[ -n a || $(echo ran > side) ]]
[ -e side ] || echo skipped-operand-not-expanded
i=0
[[ 1 == 1 || $((i=5)) ]]
echo "i=$i"
set -- a b
[[ $@ == "a b" ]] && echo joined-args
[[ -v HOME ]] && echo var-set
[[ -v no_such_var_xyz ]] || echo var-unset
[[ 1+1 -eq 2 ]] && echo arithmetic
echo then [[ a
echo hi | [[ -n a ]] && echo after-pipe
[[ -f nosuch ]] 2>/dev/null || echo redirected
f() { [[ $1 == y ]]; }
f y && echo in-function
[[ a &&
  b ]] && echo multi-line
