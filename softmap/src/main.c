#include "softmap/cmd.h"
#include "softmap/util.h"
#include "softmap/types.h"

#include <stdio.h>
#include <string.h>

#ifdef _WIN32
#include <windows.h>
#endif

static void usage(void) {
    printf("SoftMap %d.%d - software + drive tree snapshot (console)\n\n",
           SM_VERSION_MAJOR, SM_VERSION_MINOR);
    printf("Usage:\n");
    printf("  softmap scan [options]\n");
    printf("  softmap report <snapshot> [options]\n");
    printf("  softmap restore <snapshot> --target DIR [options]\n");
    printf("  softmap info <snapshot>\n\n");
    printf("Global:\n");
    printf("  -h, --help       show help\n");
    printf("  -v, --verbose    verbose log\n");
    printf("  -q, --quiet      errors only\n\n");
    printf("Defaults stay simple: scan + report summary.\n");
    printf("Extra views are opt-in (--software, --tools, --checklist, ...).\n");
}

/* Pause only when double-clicked (this process alone owns the console). */
static void pause_if_standalone_console(void) {
#ifdef _WIN32
    HANDLE hIn = GetStdHandle(STD_INPUT_HANDLE);
    DWORD mode = 0;
    /* Do not pause when stdin is redirected (scripts / pipes). */
    if (!GetConsoleMode(hIn, &mode))
        return;
    {
        DWORD pids[8];
        DWORD n = GetConsoleProcessList(pids, 8);
        if (n == 1) {
            fprintf(stderr, "\nPress Enter to close...");
            fflush(stderr);
            (void)getchar();
        }
    }
#endif
}

int main(int argc, char **argv) {
    int i = 1;
    while (i < argc) {
        if (strcmp(argv[i], "-v") == 0 || strcmp(argv[i], "--verbose") == 0) {
            sm_set_log_level(SM_LOG_VERBOSE);
            ++i;
        } else if (strcmp(argv[i], "-q") == 0 || strcmp(argv[i], "--quiet") == 0) {
            sm_set_log_level(SM_LOG_ERROR);
            ++i;
        } else if (strcmp(argv[i], "-h") == 0 || strcmp(argv[i], "--help") == 0) {
            usage();
            pause_if_standalone_console();
            return 0;
        } else {
            break;
        }
    }

    if (i >= argc) {
        usage();
        pause_if_standalone_console();
        return 1;
    }

    const char *cmd = argv[i++];
    int subc = argc - i;
    char **subv = argv + i;

    if (strcmp(cmd, "scan") == 0)
        return cmd_scan(subc, subv);
    if (strcmp(cmd, "report") == 0)
        return cmd_report(subc, subv);
    if (strcmp(cmd, "restore") == 0)
        return cmd_restore(subc, subv);
    if (strcmp(cmd, "info") == 0)
        return cmd_info(subc, subv);
    if (strcmp(cmd, "help") == 0) {
        usage();
        return 0;
    }

    sm_log(SM_LOG_ERROR, "unknown command: %s", cmd);
    usage();
    return 1;
}
