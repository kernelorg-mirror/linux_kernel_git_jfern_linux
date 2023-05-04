#!/usr/bin/python3

import re
import sys

def main(file_path):
    with open(file_path, 'r', errors='replace') as f:
        lines = f.readlines()

    cpu_trace_data = {}

    for line in lines:
        line = line.strip("\n")

        # <idle>-0       [003] d..1.     9.063487: tick_nohz_idle_stop_tick: tick_program_event: 944: 10775365981
        # match = re.match(r'\s*<.*?>-(\d+)\s+\[(\d+)\] .*tick_program_event.*: (\d+)', line)

        # <idle>-0       [001] d.h1.    10.343341: tick_event_handle: nohz_mode=lowres
        match = re.match(r'\s*<.*?>-(\d+)\s+\[(\d+)\] .*?(\d+\.\d+):.*tick_event_handle.*', line)

        if match:
            cpu = int(match.group(2))
            timestamp = float(match.group(3))
            # if cpu in cpu_trace_data:
            #    print(f"line matched {line[:-1]}, cpu {cpu} ts {timestamp} prev_ts {cpu_trace_data[cpu]}")
            # else:
            #    print(f"line matched {line[:-1]}, cpu {cpu} ts {timestamp}")

            if cpu in cpu_trace_data:
                prev_timestamp = cpu_trace_data[cpu][0]
                delta = (abs(timestamp - prev_timestamp))
                if delta < 0.0008:
                    print("=========================")
                    print(f"Problem found: delta is only {delta} prev={prev_timestamp} new={timestamp}")
                    print(f"old line: {cpu_trace_data[cpu][1]}")
                    print(f"new line: {line}")
                    print("=========================")

            cpu_trace_data[cpu] = (timestamp, line)

if __name__ == '__main__':
    if len(sys.argv) != 2:
        print("Usage: python script_name.py ftrace_file")
        sys.exit(1)

    file_path = sys.argv[1]
    main(file_path)

