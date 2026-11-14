// What calls Tondo from C in the benchmarks of `ffi`: a loop that calls a
// function given to it, on the thread that calls the loop or on threads of
// its own, which the runtime has never seen. ffi_test.to compiles it.
#include <pthread.h>

typedef long (*called)(long, long);

long calls(called function, long times) {
    long total = 0;
    for (long i = 0; i < times; i++) total = function(total, i);
    return total;
}

struct job {
    called function;
    long times, total;
    pthread_t thread;
};

static void *run(void *argument) {
    struct job *job = argument;
    job->total = calls(job->function, job->times);
    return 0;
}

long calls_from_threads(called function, long threads, long times) {
    struct job jobs[64];
    long total = 0;
    if (threads > 64) threads = 64;
    for (long t = 0; t < threads; t++) {
        jobs[t].function = function;
        jobs[t].times = times;
        pthread_create(&jobs[t].thread, 0, run, &jobs[t]);
    }
    for (long t = 0; t < threads; t++) {
        pthread_join(jobs[t].thread, 0);
        total += jobs[t].total;
    }
    return total;
}
