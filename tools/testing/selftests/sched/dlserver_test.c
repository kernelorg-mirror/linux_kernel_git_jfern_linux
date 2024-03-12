// SPDX-License-Identifier: GPL-2.0
/*
 * Use the DL server infrastructure to give CFS tasks a fixed bandwidth.
 *
 * Copyright (c) 2024 Google.
 * Author: Joel Fernandes <joel@joelfernandes.org>
 *
 * This library is free software; you can redistribute it and/or modify it
 * under the terms of version 2.1 of the GNU Lesser General Public License as
 * published by the Free Software Foundation.
 *
 * This library is distributed in the hope that it will be useful, but WITHOUT
 * ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or
 * FITNESS FOR A PARTICULAR PURPOSE.  See the GNU Lesser General Public License
 * for more details.
 *
 * You should have received a copy of the GNU Lesser General Public License
 * along with this library; if not, see <http://www.gnu.org/licenses>.
 */

#define _GNU_SOURCE

#include <stdlib.h>
#include <unistd.h>
#include <sched.h>
#include <time.h>
#include <sys/wait.h>
#include <sys/types.h>
#include <sys/prctl.h>
#include <fcntl.h>
#include <string.h>

#include "common.h"

enum pid_type {PIDTYPE_PID = 0, PIDTYPE_TGID, PIDTYPE_PGID};

#define RUN_TIME 4UL // Running time of the test in seconds
#define CPU_ID 0 // Assuming we're pinning processes to the first core
#define DL_SERVER_DEBUGFS "/sys/kernel/debug/sched/fair_server"

void write_server_debugfs(char *file, char *type, unsigned long value)
{
	char path[1024], buf[1024];
	int fd, n;

	snprintf(path, sizeof(path), "%s/%s/%s", DL_SERVER_DEBUGFS, file, type);
	fd = open(path,	O_WRONLY);
	if (fd == -1) {
		perror("Failed to open file for writing");
		return;
	}
	n = snprintf(buf, sizeof(buf), "%lu\n", value);
	n = write(fd, buf, n);
	if (n == -1)
		perror("Failed to write file");

	close(fd);
}

void write_dl_server_params(void)
{
	DIR *dir;
	struct dirent *entry;

	if (access(DL_SERVER_DEBUGFS, F_OK) == -1) {
		perror("DL server debugfs not found, cannot set DL parameters.");
		exit(EXIT_FAILURE);
	}

	dir = opendir(DL_SERVER_DEBUGFS);
	if (dir	== NULL) {
		perror("Failed to open directory");
		exit(EXIT_FAILURE);
	}

	while ((entry = readdir(dir)) != NULL) {
		if (strcmp(entry->d_name, ".") == 0 || strcmp(entry->d_name, "..") == 0)
			continue;

		write_server_debugfs(entry->d_name, "period",  50000000);
		write_server_debugfs(entry->d_name, "runtime", 25000000);
	}
	closedir(dir);
}

void process_func(void)
{
	unsigned long long count = 0;
	time_t end;

	// Busy loop for RUN_TIME seconds
	end = time(NULL) + RUN_TIME;
	while (time(NULL) < end) {
		count++;
	}
}

struct timespec timespec_diff(struct timespec start, struct timespec end)
{
    struct timespec temp;
    if ((end.tv_nsec - start.tv_nsec) < 0) {
        temp.tv_sec = end.tv_sec - start.tv_sec - 1;
        temp.tv_nsec = 1000000000 + end.tv_nsec - start.tv_nsec;
    } else {
        temp.tv_sec = end.tv_sec - start.tv_sec;
        temp.tv_nsec = end.tv_nsec - start.tv_nsec;
    }
    return temp;
}

unsigned long timespec_to_nsec(struct timespec ts)
{
    return (ts.tv_sec * 1000000000) + ts.tv_nsec;
}

void process_func_periodic(int delay_ms, int run_ms, int sleep_ms)
{
	unsigned long long count = 0;
	struct timespec start, cur, sleep_ts, start_run, cur_run;
    unsigned long run_and_sleep;

    // Delay for delay_ms
    clock_gettime(CLOCK_MONOTONIC, &start);
    sleep_ts = (struct timespec) {delay_ms / 1000, (delay_ms % 1000) * 1000000};
    nanosleep(&sleep_ts, NULL);
    clock_gettime(CLOCK_MONOTONIC, &cur);
    printf("Delay for sec=%lf\n", timespec_to_nsec(timespec_diff(start, cur)) / 1000000000.0);

    run_and_sleep = RUN_TIME * 3 / 4;
    run_and_sleep *= 1000000000; // Convert to nanoseconds

    clock_gettime(CLOCK_MONOTONIC, &start);
    while (clock_gettime(CLOCK_MONOTONIC, &cur) == 0 &&
           timespec_to_nsec(timespec_diff(start, cur)) < run_and_sleep) {
        bool only_run = false;
        unsigned long run_duration = run_ms * 1000000;
        unsigned long sleep_duration = sleep_ms * 1000000;
        unsigned long time_to_end = run_and_sleep - timespec_to_nsec(timespec_diff(start, cur));

        if (time_to_end < run_duration) {
            run_duration = time_to_end;
            only_run = true;
        }

        clock_gettime(CLOCK_MONOTONIC, &start_run);
        while(clock_gettime(CLOCK_MONOTONIC, &cur_run) == 0 &&
              timespec_to_nsec(timespec_diff(start_run, cur_run)) < run_duration) {
            count++;
        }

        if (only_run)
            break;
        clock_gettime(CLOCK_MONOTONIC, &cur);
        time_to_end = run_and_sleep - timespec_to_nsec(timespec_diff(start, cur));
        if (time_to_end < sleep_duration)
            sleep_duration = time_to_end;
        nanosleep(&(struct timespec) {sleep_duration / 1000000000, sleep_duration % 1000000000}, NULL);
    }

    ksft_print_msg("Exiting PID %d\n", getpid());
}

void set_affinity(int cpu_id)
{
	cpu_set_t cpuset;

	CPU_ZERO(&cpuset);
	CPU_SET(cpu_id, &cpuset);

	if (sched_setaffinity(0, sizeof(cpu_set_t), &cpuset) != 0) {
		perror("sched_setaffinity");
		exit(EXIT_FAILURE);
	}
}

void set_sched(int policy, int priority)
{
	struct sched_param param;

	param.sched_priority = priority;
	if (sched_setscheduler(0, policy, &param) != 0) {
		perror("sched_setscheduler");
		exit(EXIT_FAILURE);
	}
}

float get_process_runtime(int pid)
{
	char path[256];
	FILE *file;
	long utime, stime;
	int fields;

	snprintf(path, sizeof(path), "/proc/%d/stat", pid);
	file = fopen(path, "r");
	if (file == NULL) {
		perror("Failed to open stat file");
		return -1; // Indicate failure
	}

	// Skip the first 13 fields and read the 14th and 15th
	fields = fscanf(file,
					"%*d %*s %*c %*d %*d %*d %*d %*d %*u %*u %*u %*u %*u %lu %lu",
					&utime, &stime);
	fclose(file);

	if (fields != 2) {
		fprintf(stderr, "Failed to read stat file\n");
		return -1; // Indicate failure
	}

	// Calculate the total time spent in the process
	long total_time = utime + stime;
	long ticks_per_second = sysconf(_SC_CLK_TCK);
	float runtime_seconds = total_time * 1.0 / ticks_per_second;

	return runtime_seconds;
}

int main(void)
{
	float runtime1;
    float runtime_expected;
	int pid4, pid5;

	write_dl_server_params();

	ksft_print_header();
	ksft_set_plan(1);

    pid4 = fork();
    if (pid4 == 0) {
        set_affinity(CPU_ID);
        process_func_periodic(10, 15, 35); // delay 10ms, run 15ms, sleep 35ms (~30% bw)
        exit(0);
    } else if (pid4 < 0) {
        perror("fork for p4");
        ksft_exit_fail();
    }

    pid5 = fork();
    if (pid5 == 0) {
        set_affinity(CPU_ID);
        set_sched(SCHED_RR, 50);
        // delay 50ms, run 25ms, sleep 5ms (~83% bw so force CFS get some).
        process_func_periodic(50, 25, 0);
        exit(0);
    } else if (pid5 < 0) {
        perror("fork for p4");
        ksft_exit_fail();
    }

    sleep(RUN_TIME * 3 / 4);

    runtime1 = get_process_runtime(pid4);
    if (runtime1 != -1)
        ksft_print_msg("Runtime of PID %d is %f seconds\n", pid4, runtime1);
    else
        ksft_exit_fail_msg("Error getting runtime for PID %d\n", pid4);

    // CFS task should have got 30% of the bandwidth
    runtime_expected = 0.3 * (RUN_TIME * 3) / 4;
    if (runtime1 < 0.9 * runtime_expected || runtime1 > 1.1 * runtime_expected)
        ksft_exit_fail_msg("Runtime of PID %d is not within 10%% of expected runtime %f\n",
                           pid4, runtime_expected);

    waitpid(pid4, NULL, 0);
    waitpid(pid5, NULL, 0);

	ksft_test_result_pass("PASS\n");
	return 0;
}
