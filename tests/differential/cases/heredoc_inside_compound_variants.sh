name=world
if true; then cat <<A; cat <<B
first $name
A
second $name
B
fi
if true; then
	cat <<-EOF
	tabs $name
	EOF
fi
{ cat <<'EOF'
quoted $name
EOF
}
for i in 1; do cat <<'EOF' | tr a-z A-Z
piped $name
EOF
done
echo after
