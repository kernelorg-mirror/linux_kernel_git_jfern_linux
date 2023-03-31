#!/bin/bash

id=$(id -u)
if [ "$id" != 0 ]; then
	echo "Run as root"
	exit
fi

function cleanup {
	echo "Exiting script, killing child processes..."
	kill $pid1 $pid2 $pid3 $pid4 $pid5
	echo "deleting all created cgroups.."
	sudo cgdelete -r cpu:L1_C1
	exit 1
}

trap cleanup EXIT

# Create cgroups as follows
#     ROOT
#       |
#      RT1
#     /   \
#   RT2    RT3
#         /   \
#        RT4   RT5

CG1=L1_C1
CG2=L1_C1/L2_C1
CG3=L1_C1/L2_C2
CG4=L1_C1/L2_C2/L3_C1
CG5=L1_C1/L2_C2/L3_C2

cpu_rt_runtime_cg1="/sys/fs/cgroup/cpu/$CG1/cpu.rt_runtime_us"
cpu_rt_runtime_cg2="/sys/fs/cgroup/cpu/$CG2/cpu.rt_runtime_us"
cpu_rt_runtime_cg3="/sys/fs/cgroup/cpu/$CG3/cpu.rt_runtime_us"
cpu_rt_runtime_cg4="/sys/fs/cgroup/cpu/$CG4/cpu.rt_runtime_us"
cpu_rt_runtime_cg5="/sys/fs/cgroup/cpu/$CG5/cpu.rt_runtime_us"

tasks_cg1="/sys/fs/cgroup/cpu/$CG1/tasks"
tasks_cg2="/sys/fs/cgroup/cpu/$CG2/tasks"
tasks_cg3="/sys/fs/cgroup/cpu/$CG3/tasks"
tasks_cg4="/sys/fs/cgroup/cpu/$CG4/tasks"
tasks_cg5="/sys/fs/cgroup/cpu/$CG5/tasks"

sudo cgcreate -g cpu:$CG1
sudo cgcreate -g cpu:$CG2
sudo cgcreate -g cpu:$CG3
sudo cgcreate -g cpu:$CG4
sudo cgcreate -g cpu:$CG5


echo "950000" | sudo tee $cpu_rt_runtime_cg1

echo "300000" | sudo tee $cpu_rt_runtime_cg2

echo "500000" | sudo tee $cpu_rt_runtime_cg3

echo "200000" | sudo tee $cpu_rt_runtime_cg4

echo "250000" | sudo tee $cpu_rt_runtime_cg5

#sudo timeout -k 20 15 cgexec -g cpu:$CG1 chrt -f 90 ./hackbench -f 10 -l 1000000 > /dev/null 2>&1 &
#sudo timeout -k 20 15 cgexec -g cpu:$CG2 chrt -f 91 ./hackbench -f 10 -l 1000000 > /dev/null 2>&1  &
#sudo timeout -k 20 15 cgexec -g cpu:$CG3 chrt -f 92 ./hackbench -f 10 -l 1000000 > /dev/null 2>&1 &
pid=0
function busy_loop()
{
	set -e
	cmask=$1
	cg=$2
	tasks_cg=$3
	sh -c 'while true; do :; done' &
	pid=$!
	taskset -p $cmask $pid
	echo $pid > $tasks_cg
	chrt -f -p 99 $pid
	set +e
	echo $pid
}

#busy_loop $CG1 $tasks_cg1
#pid1=$!
#echo "$pid1 moved to cgroup $CG1 and busy looping.."

busy_loop 1 $CG2 $tasks_cg2
pid2=$!
echo "$pid2 moved to cgroup $CG2 and busy looping.."

busy_loop 2 $CG3 $tasks_cg3
pid3=$!
echo "$pid3 moved to cgroup $CG3 and busy looping.."

busy_loop 4 $CG4 $tasks_cg4
pid4=$!
echo "$pid4 moved to cgroup $CG4 and busy looping.."

busy_loop 8 $CG5 $tasks_cg5
pid5=$!
echo "$pid5 moved to cgroup $CG5 and busy looping.."

while true; do
	sleep 5
	#c1=$(ps -p $pid1 -o %cpu | awk 'NR>1 {print $1}')
	c2=$(ps -p $pid2 -o %cpu | awk 'NR>1 {print $1}')
	c3=$(ps -p $pid3 -o %cpu | awk 'NR>1 {print $1}')
	c4=$(ps -p $pid4 -o %cpu | awk 'NR>1 {print $1}')
	c5=$(ps -p $pid5 -o %cpu | awk 'NR>1 {print $1}')
	echo "Cpu Usage P1($c1%), P2($c2%), P3($c3%), P4($c4%), P5($c5%)"
done

exit

while true; do
    # Spawn first process with real-time priority and pin to CPU 2
    # taskset -c 2 chrt -f 99 sh -c 'while true; do :; done' &
    sh -c 'while true; do :; done' &

    # Get the process ID of the first process
    pid1=$!
    echo $pid1

    # Set real-time priority for first process
    chrt -f -p 99 $pid1

    # Spawn second process with real-time priority and pin to CPU 2
    sh -c 'while true; do :; done' &

    # Get the process ID of the second process
    pid2=$!
    echo $pid2

    # Set real-time priority for second process
    chrt -f -p 99 $pid2

    # Wait for a few seconds to let the processes run
    sleep 5

    # Check if each process used at most 90% of CPU time
    cpu_usage1=$(ps -p $pid1 -o %cpu | awk 'NR>1 {print $1}')
    cpu_usage2=$(ps -p $pid2 -o %cpu | awk 'NR>1 {print $1}')
    if (( $(echo "$cpu_usage1 <= 96.0" | bc -l) )) && (( $(echo "$cpu_usage2 <= 96.0" | bc -l) )); then
        echo "Success: Process 1 CPU usage = $cpu_usage1%, Process 2 CPU usage = $cpu_usage2%"
    else
        echo "Error: Process 1 CPU usage = $cpu_usage1%, Process 2 CPU usage = $cpu_usage2%"
    fi

    # Kill both processes and start over
    kill $pid1 $pid2
done

