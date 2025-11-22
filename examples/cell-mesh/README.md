# Run mesh
cell mitosis .

# See output
tail -f run/service.log

# Check how lightweight this is despite running 50 cells.
htop -p $(pgrep -d, chatterbox)