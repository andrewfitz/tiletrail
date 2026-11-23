/* Exercise the public C ABI on a long, four-tile synthetic history. */
#define _POSIX_C_SOURCE 200809L
#include "tiletrail.h"
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

static void check(int status, const tiletrail_error_t *error) {
    if (status != TILETRAIL_OK) {
        fprintf(stderr, "%s\n", error->message);
        exit(EXIT_FAILURE);
    }
}

static double now(void) {
    struct timespec time;
    if (clock_gettime(CLOCK_MONOTONIC, &time) != 0) {
        perror("clock_gettime");
        exit(EXIT_FAILURE);
    }
    return time.tv_sec + time.tv_nsec / 1e9;
}

int main(void) {
    enum { width = 48, height = 32, frames = 60000 };
    uint8_t pixels[width * height * 3] = {0};
    const uint8_t colors[][3] = {{209, 22, 40}, {35, 210, 40},
                                 {30, 50, 219}, {201, 168, 36}};
    for (int y = 0; y < height; ++y) {
        for (int x = 0; x < width; ++x) {
            if ((x >= 22 && x <= 25) || (y >= 14 && y <= 17)) {
                continue;
            }
            const uint8_t *color = colors[(y >= height / 2) * 2 + (x >= width / 2)];
            for (int channel = 0; channel < 3; ++channel) {
                pixels[(y * width + x) * 3 + channel] = color[channel];
            }
        }
    }

    tiletrail_config_t config = {TILETRAIL_MODE_CPU_ONLY, TILETRAIL_SAMPLING_EVERY_FRAME};
    tiletrail_timestamp_t time = {0, 1, 30};
    tiletrail_analyzer_t *analyzer = NULL;
    tiletrail_error_t error;
    check(tiletrail_analyzer_create(&config, time, &analyzer, &error), &error);
    tiletrail_frame_t frame = {
        .data = pixels, .len = sizeof(pixels), .stride = width * 3,
        .width = width, .height = height, .format = TILETRAIL_FORMAT_RGB24,
        .pixel_aspect_num = 1, .pixel_aspect_den = 1, .timestamp = time
    };
    double start = now();
    for (int index = 0; index < frames; ++index) {
        frame.timestamp.pts = index;
        check(tiletrail_analyzer_push(analyzer, &frame, &error), &error);
    }
    time.pts = frames;
    tiletrail_buffer_t result = {0};
    double pushed = now();
    check(tiletrail_analyzer_result_json(analyzer, time, &result, &error), &error);
    double finished = now();

    /* Hash after timing, directly over the independently owned result. */
    uint64_t hash = UINT64_C(14695981039346656037);
    for (uint64_t index = 0; index < result.len; ++index) {
        hash ^= result.data[index];
        hash *= UINT64_C(1099511628211);
    }
    printf("frames=%d json=%" PRIu64 " hash=%" PRIx64
           " push=%.4f finish=%.4f total=%.4f\n", frames, result.len, hash,
           pushed - start, finished - pushed, finished - start);
    tiletrail_analyzer_free(analyzer);
    tiletrail_buffer_free(&result);
    return 0;
}
