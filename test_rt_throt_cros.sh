#!/bin/bash

id=$(id -u)
if [ "$id" != 0 ]; then
	echo "Run as root"
	exit
fi

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

cg1_dir="/sys/fs/cgroup/cpu/$CG1"
cg2_dir="/sys/fs/cgroup/cpu/$CG2"
cg3_dir="/sys/fs/cgroup/cpu/$CG3"
cg4_dir="/sys/fs/cgroup/cpu/$CG4"
cg5_dir="/sys/fs/cgroup/cpu/$CG5"

cpu_rt_runtime_cg1="$cg1_dir/cpu.rt_runtime_us"
cpu_rt_runtime_cg2="$cg2_dir/cpu.rt_runtime_us"
cpu_rt_runtime_cg3="$cg3_dir/cpu.rt_runtime_us"
cpu_rt_runtime_cg4="$cg4_dir/cpu.rt_runtime_us"
cpu_rt_runtime_cg5="$cg5_dir/cpu.rt_runtime_us"

tasks_cg1="$cg1_dir/tasks"
tasks_cg2="$cg2_dir/tasks"
tasks_cg3="$cg3_dir/tasks"
tasks_cg4="$cg4_dir/tasks"
tasks_cg5="$cg5_dir/tasks"

function cleanup {
	echo "Exiting script, killing child processes..."
	kill $pid1 $pid2 $pid3 $pid4 $pid5
	sleep 2;
	echo "deleting all created cgroups.."
	rmdir $cg5_dir
	rmdir $cg4_dir
	rmdir $cg3_dir
	rmdir $cg2_dir
	rmdir $cg1_dir
	exit 1
}

trap cleanup EXIT

mkdir $cg1_dir
mkdir $cg2_dir
mkdir $cg3_dir
mkdir $cg4_dir
mkdir $cg5_dir


echo "950000" | tee $cpu_rt_runtime_cg1

echo "300000" | tee $cpu_rt_runtime_cg2

echo "500000" | tee $cpu_rt_runtime_cg3

echo "200000" | tee $cpu_rt_runtime_cg4

echo "250000" | tee $cpu_rt_runtime_cg5

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

