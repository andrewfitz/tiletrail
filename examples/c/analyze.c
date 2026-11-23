/* Feed packed RGB24 frames from a raw sequence into the TileTrail C ABI. */
#include "tiletrail.h"

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void report(const char *operation, int32_t code, const tiletrail_error_t *error)
{
    fprintf(stderr, "%s failed (%d): %s\n", operation, (int)code, error->message);
}

static int parse_positive(const char *text, uint32_t *output)
{
    char *end;
    errno = 0;
    if (*text < '0' || *text > '9') {
        return 0;
    }
    unsigned long long value = strtoull(text, &end, 10);
    if (errno || *end || !value || value > UINT32_MAX) {
        return 0;
    }
    *output = (uint32_t)value;
    return 1;
}

int main(int argc, char **argv)
{
    if (argc != 7) {
        fprintf(stderr, "usage: %s frames.rgb width height first_pts tick_numerator tick_denominator\n",
                argv[0]);
        return EXIT_FAILURE;
    }

    uint32_t width, height;
    tiletrail_timestamp_t time = {0};
    char *end;
    errno = 0;
    time.pts = (int64_t)strtoll(argv[4], &end, 10);
    if (errno || end == argv[4] || *end ||
        !parse_positive(argv[2], &width) || !parse_positive(argv[3], &height) ||
        !parse_positive(argv[5], &time.numerator) ||
        !parse_positive(argv[6], &time.denominator) || SIZE_MAX / width / 3 < height) {
        fprintf(stderr, "invalid frame dimensions or timestamp\n");
        return EXIT_FAILURE;
    }
    size_t stride = (size_t)width * 3;
    size_t len = stride * height;
    int result = EXIT_FAILURE;
    FILE *input = fopen(argv[1], "rb");
    uint8_t *data = malloc(len);
    tiletrail_analyzer_t *analyzer = NULL;
    tiletrail_buffer_t output = {0};
    tiletrail_buffer_t markers = {0};

    if (!input || !data) {
        fprintf(stderr, "could not open input or allocate frame buffer\n");
        goto done;
    }
    if (tiletrail_abi_version() != TILETRAIL_ABI_VERSION) {
        fprintf(stderr, "ABI version mismatch\n");
        goto done;
    }

    tiletrail_config_t config = {TILETRAIL_MODE_CPU_ONLY, TILETRAIL_SAMPLING_EVERY_FRAME};
    tiletrail_error_t error = {0};
    int32_t code = tiletrail_analyzer_create(&config, time, &analyzer, &error);
    if (code != TILETRAIL_OK) {
        report("create", code, &error);
        goto done;
    }
    tiletrail_frame_t frame = {
        data, len, stride, width, height, TILETRAIL_FORMAT_RGB24, 0, 1, 1, time
    };

    /* Confirm length validation before committing this timestamp. */
    memset(data, 0, len);
    frame.len = len - 1;
    code = tiletrail_analyzer_push(analyzer, &frame, &error);
    if (code != TILETRAIL_INVALID_FRAME || error.code != code ||
        !error.has_resume || error.resume_at.pts != time.pts || !error.message[0]) {
        report("validate truncated frame", code, &error);
        goto done;
    }
    frame.len = len;

    for (;;) {
        size_t count = fread(data, 1, len, input);
        if (!count && feof(input) && !ferror(input)) {
            break;
        }
        if (count != len || time.pts == INT64_MAX) {
            fprintf(stderr, "truncated frame, read error, or timestamp overflow\n");
            goto done;
        }
        frame.timestamp = time;
        code = tiletrail_analyzer_push(analyzer, &frame, &error);
        if (code != TILETRAIL_OK || error.code != TILETRAIL_OK) {
            report("push", code, &error);
            goto done;
        }
        ++time.pts;
    }
    memset(data, 0, len);
    code = tiletrail_analyzer_result_json(analyzer, time, &output, &error);
    if (code != TILETRAIL_OK) {
        report("snapshot", code, &error);
        goto done;
    }

    /* The snapshot belongs to the caller, independent of analyzer ownership. */
    tiletrail_analyzer_free(analyzer);
    analyzer = NULL;
    static const uint8_t marker_rule[] =
        "{\"unit\":\"participants\",\"threshold\":\"any\",\"kinds\":[\"enter\"],"
        "\"roster\":null,\"settling_window_ms\":200}";
    code = tiletrail_scene_markers_json(output.data, output.len, marker_rule,
                                       sizeof marker_rule - 1, &markers, &error);
    if (code != TILETRAIL_OK || !markers.data || markers.len < 2 || markers.data[0] != '[') {
        report("query saved markers", code, &error);
        goto done;
    }
    if (!output.data || !output.len || fwrite(output.data, 1, (size_t)output.len, stdout) != output.len ||
        fflush(stdout) != 0) {
        fprintf(stderr, "could not write result JSON\n");
        goto done;
    }
    tiletrail_buffer_free(&output);
    if (output.data || output.len) {
        fprintf(stderr, "buffer was not cleared after release\n");
        goto done;
    }
    result = EXIT_SUCCESS;

done:
    tiletrail_buffer_free(&markers);
    tiletrail_buffer_free(&output);
    tiletrail_analyzer_free(analyzer);
    free(data);
    if (input) {
        fclose(input);
    }
    return result;
}
