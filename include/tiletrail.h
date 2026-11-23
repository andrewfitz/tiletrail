#ifndef TILETRAIL_H
#define TILETRAIL_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define TILETRAIL_ABI_VERSION 1u

typedef struct tiletrail_analyzer tiletrail_analyzer_t;

enum {
    TILETRAIL_OK = 0,
    TILETRAIL_INVALID_FRAME = 1,
    TILETRAIL_INVALID_TIMESTAMP = 2,
    TILETRAIL_INVALID_RANGE = 3,
    TILETRAIL_GPU_UNAVAILABLE = 4,
    TILETRAIL_DEVICE_LOST = 5,
    TILETRAIL_INVALID_ARGUMENT = 6,
    TILETRAIL_PANIC = 7,
    TILETRAIL_SERIALIZATION_ERROR = 8
};

enum {
    TILETRAIL_MODE_AUTO = 0,
    TILETRAIL_MODE_CPU_ONLY = 1,
    TILETRAIL_MODE_GPU_REQUIRED = 2
};

enum {
    TILETRAIL_SAMPLING_EVERY_FRAME = 0,
    TILETRAIL_SAMPLING_SPARSE = 1
};

enum {
    TILETRAIL_FORMAT_GRAY8 = 0,
    TILETRAIL_FORMAT_RGB24 = 1,
    TILETRAIL_FORMAT_RGBA8 = 2,
    TILETRAIL_FORMAT_BGRA8 = 3
};

typedef struct {
    int64_t pts;
    uint32_t numerator;
    uint32_t denominator;
} tiletrail_timestamp_t;

typedef struct {
    uint32_t mode;
    uint32_t sampling;
} tiletrail_config_t;

typedef struct {
    const uint8_t *data;
    uint64_t len;
    uint64_t stride;
    uint32_t width;
    uint32_t height;
    uint32_t format;
    uint32_t rotation;
    uint32_t pixel_aspect_num;
    uint32_t pixel_aspect_den;
    tiletrail_timestamp_t timestamp;
} tiletrail_frame_t;

typedef struct {
    int32_t code;
    uint32_t has_resume;
    tiletrail_timestamp_t resume_at;
    char message[256];
} tiletrail_error_t;

typedef struct {
    const uint8_t *data;
    uint64_t len;
} tiletrail_buffer_t;

uint32_t tiletrail_abi_version(void);

/* All pointers must be valid, aligned, and non-overlapping for the duration
 * of each call, except where NULL is allowed. Calls on a given analyzer
 * (including free) must be serialized by the caller. An error output is
 * optional (NULL); when supplied, it is reset on success. The diagnostic
 * message is NUL-terminated UTF-8 and may be truncated. The resume_at field
 * is present only when has_resume is nonzero. Release an analyzer whose
 * push call returns TILETRAIL_PANIC.
 *
 * Pass NULL config for the default configuration. The out pointer must
 * point to storage that does not own a live handle. Failure sets *out to NULL.
 */
int32_t tiletrail_analyzer_create(const tiletrail_config_t *config,
                                 tiletrail_timestamp_t start,
                                 tiletrail_analyzer_t **out,
                                 tiletrail_error_t *error);

/* Borrow the frame and its len readable bytes only until this call returns.
 * Rotation is clockwise in degrees (0, 90, 180, 270).
 */
int32_t tiletrail_analyzer_push(tiletrail_analyzer_t *analyzer,
                               const tiletrail_frame_t *frame,
                               tiletrail_error_t *error);

/* Write an independently owned UTF-8 JSON snapshot, with no trailing NUL.
 * The end timestamp is exclusive. Further pushes are permitted afterwards.
 * The out struct must not own an unreleased buffer. Failure clears *out.
 * A successful buffer remains valid after the analyzer is freed.
 */
int32_t tiletrail_analyzer_result_json(const tiletrail_analyzer_t *analyzer,
                                      tiletrail_timestamp_t end,
                                      tiletrail_buffer_t *out,
                                      tiletrail_error_t *error);

/* Query scene markers from a saved JSON result and a JSON marker rule.
 * The input buffers are borrowed until return. Output ownership and failure
 * behavior match tiletrail_analyzer_result_json.
 */
int32_t tiletrail_scene_markers_json(const uint8_t *analysis_json,
                                     uint64_t analysis_len,
                                     const uint8_t *rule_json,
                                     uint64_t rule_len,
                                     tiletrail_buffer_t *out,
                                     tiletrail_error_t *error);

/* NULL is allowed. Each non-NULL handle must be released exactly once. */
void tiletrail_analyzer_free(tiletrail_analyzer_t *analyzer);

/* Release the returned allocation and zero the struct. NULL and a zeroed
 * struct are allowed. Do not release copies or edit ownership fields.
 */
void tiletrail_buffer_free(tiletrail_buffer_t *buffer);

#ifdef __cplusplus
}
#endif

#endif
