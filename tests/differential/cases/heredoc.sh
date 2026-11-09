name=world
cat <<EOT
hello $name
sum=$((1 + 2))
EOT
cat <<'EOT'
literal $name
EOT
	cat <<-EOT
	tab stripped
	EOT
