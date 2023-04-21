/*
 * This program is free software; you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation; either version 2 of the License, or
 * (at your option) any later version.
 *
 * Author: Joel Fernandes <joel@joelfernandes.org>
 *
 * This program is a top-like utility for RCU statistics and debugging.
 */
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <string.h>
#include <signal.h>

struct rcu_debug_entry
{
	char func_name[64];
	int max_wait_ms;
	int total_execs;
};

struct rcu_debug_entry_node
{
	char func_name[64];
	int max_wait_ms;
	int total_execs;
	struct rcu_debug_entry_node *next;
};

// Global linked list head of rcu_debug_entry_nodes.
struct rcu_debug_entry_node *func_head;

// Function to add a new entry to the linked list
void add_rcu_debug_entry_func(struct rcu_debug_entry *rcu_debug_entry)
{
	struct rcu_debug_entry_node *new_node = malloc(sizeof(struct rcu_debug_entry_node));
	if (new_node == NULL)
		return;

	strcpy(new_node->func_name, rcu_debug_entry->func_name);
	new_node->max_wait_ms = rcu_debug_entry->max_wait_ms;
	new_node->total_execs = rcu_debug_entry->total_execs;
	new_node->next = func_head;
	func_head = new_node;
}

// Function to find an entry in the linked list given the function name
struct rcu_debug_entry_node *find_rcu_debug_entry_func(char *func_name)
{
	struct rcu_debug_entry_node *tmp = func_head;
	while (tmp) {
		if (strcmp(tmp->func_name, func_name) == 0)
			return tmp;
		tmp = tmp->next;
	}
	return NULL;
}

// Function to print all the entries in the linked list
void print_rcu_debug_entries_func(void)
{
	struct rcu_debug_entry_node *tmp = func_head;
	// Print the table header with equal spacing
	printf("%-40s%-20s%-20s\n", "Function Name", "Max Wait (ms)", "Total Execs");

	// Print a dashed line to separate the header from the data
	// Dashed line is same length as the header
	printf("%-40s%-20s%-20s\n", "----------------------------------------", "--------------------", "--------------------");

	while (tmp) {
		// Format it as a table correctly with equal spacing
		printf("%-40s%-20d%-20d\n", tmp->func_name, tmp->max_wait_ms, tmp->total_execs);
		tmp = tmp->next;
	}
}

struct rcu_debug_entry *rcu_debug_entries[2048];

// Function to free the debug entries
void free_rcu_debug_entries(void)
{
	for (int i = 0; i < 2048; i++) {
		if (rcu_debug_entries[i]) {
			free(rcu_debug_entries[i]);
			rcu_debug_entries[i] = NULL;
		}
	}
}

// Read the /sys/kernel/debug/rcu/callbacks file and store the entries in an
// array of rcu_debug_entry structs. Each array is a part of a larger array of
// CPUs.
//
// The callbacks file is of the format (example):
// cpu: 2, rcu_debug_ptrs_nr: 2
// ip: delayed_put_task_struct+0x0/0xa0, max_wait_ms: 13, total_execs: 1066
// ip: delayed_put_pid+0x0/0x20, max_wait_ms: 13, total_execs: 1065
// cpu: 3, rcu_debug_ptrs_nr: 1
// ip: rcu_free_wq+0x0/0x40, max_wait_ms: 10, total_execs: 8
int read_rcu_callbacks(void)
{
	FILE *fp;
	char *line = NULL;
	size_t len = 0;
	ssize_t read;
	int cpu = -1, rcu_debug_ptrs_nr = -1;
	int i = 0, j = 0;

	fp = fopen("/sys/kernel/debug/rcu/callbacks", "r");
	if (fp == NULL)
		return -1;

	while ((read = getline(&line, &len, fp)) != -1) {
		if (line[0] == 'c') {
			sscanf(line, "cpu: %d, rcu_debug_ptrs_nr: %d", &cpu, &rcu_debug_ptrs_nr);
			if (cpu == -1 || rcu_debug_ptrs_nr == -1)
				return -1;
			// TODO: Fix this to allocate only rcu_debug_ptrs_nr. For now its
			// Ok as we don't have more than 2048 callbacks.
			rcu_debug_entries[cpu] = malloc(2048 * sizeof(struct rcu_debug_entry));
			if (rcu_debug_entries[cpu] == NULL)
				return -1;
			i = 0;
		} else if (line[0] == 'i') {
			sscanf(line, "ip: %s max_wait_ms: %d, total_execs: %d",
				rcu_debug_entries[cpu][i].func_name,
				&rcu_debug_entries[cpu][i].max_wait_ms,
				&rcu_debug_entries[cpu][i].total_execs);

			// Set last char of func_name to NULL char since it absorbs the comma
			rcu_debug_entries[cpu][i].func_name[strlen(rcu_debug_entries[cpu][i].func_name) - 1] = '\0';
			i++;
		}
	}

	fclose(fp);
	if (line)
		free(line);

	return 0;
}

// Function to build a list of rcu_debug_entry_node structs. Each node in the
// list is a function name, the maximum of all the max_wait_ms across all CPUs,
// and the sum of all the total_execs across all CPUs.
int read_rcu_callbacks_list(void)
{
	for (int i = 0; i < 2048; i++) {
		if (rcu_debug_entries[i]) {
			for (int j = 0; j < 2048; j++) {
				if (rcu_debug_entries[i][j].func_name[0] == '\0')
					continue;
				struct rcu_debug_entry_node *tmp = find_rcu_debug_entry_func(rcu_debug_entries[i][j].func_name);
				if (tmp) {
					if (tmp->max_wait_ms < rcu_debug_entries[i][j].max_wait_ms)
						tmp->max_wait_ms = rcu_debug_entries[i][j].max_wait_ms;
					tmp->total_execs += rcu_debug_entries[i][j].total_execs;
				} else {
					add_rcu_debug_entry_func(&rcu_debug_entries[i][j]);
				}
			}
		}
	}

	return 0;
}

// Function to handle Ctrl-C
void sigint_handler(int signum)
{
	free_rcu_debug_entries();
	exit(0);
}

int main(void)
{
	// Handle Ctrl-C
	signal(SIGINT, sigint_handler);

	while (1) {
		system("clear");

		if (read_rcu_callbacks() == -1)
			goto cont;
		if (read_rcu_callbacks_list() == -1)
			goto cont;
		print_rcu_debug_entries_func();
cont:
		free_rcu_debug_entries();
		sleep(5);
	}

	return 0;
}
