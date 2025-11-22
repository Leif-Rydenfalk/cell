# Run mesh
cell mitosis .

# See output
tail -f run/service.log

# Check how lightweight this is despite running 50 cells.
htop -p $(pgrep -d, chatterbox)

ls -lh run/service.log

# Usage summary (long output)
ps -o pid,%cpu,%mem,rss,comm -p $(pgrep -d, chatterbox)

# Exact cpu usage of 'chatterbox' in MB
ps -o rss -p $(pgrep -d, chatterbox) | awk '{s+=$1} END {print s/1024 " MB"}'