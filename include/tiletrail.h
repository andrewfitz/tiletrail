#ifndef TILETRAIL_H
#define TILETRAIL_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

#define TILETRAIL_ABI_VERSION 1u
#define TILETRAIL_HAS_FEED_LAYOUTS 1
#define TILETRAIL_HAS_PROGRESS 1
#define TILETRAIL_HAS_COARSE_PUSH 1
#define TILETRAIL_HAS_PREPARED_FRAMES 1
#define TILETRAIL_HAS_CHECKPOINTS 1
#define TILETRAIL_HAS_RESULT_CHUNKS 1
/* Frames whose longer side is at most this many pixels are measured pixel for
 * pixel. A larger frame is point-sampled to that size: its longer side becomes
 * the limit, the other side scales to match rounded to nearest (at least one),
 * and output pixel i of n reads source pixel ((2 * i + 1) * source) / (2 * n).
 * A caller may submit a frame it has already sampled this way, with the
 * source's rotation and pixel aspect, and obtains the same result. */
#define TILETRAIL_SAMPLE_LIMIT 256u
#define TILETRAIL_FEED_DETECTOR_REVISION 2u
/* Changes whenever meeting analysis can return different regions or
 * identities for the same frames. Absent before revision 2, which holds
 * established video tiles through brief cuts and merges by the detector. */
#define TILETRAIL_MEETING_DETECTOR_REVISION 2u

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

/* Analyze arbitrary video feeds. Pass (0, 0) to detect feed boxes or supply
 * a positive, uniform grid of up to 64 cells. Columns and rows refer to
 * display orientation. Each fixed-grid cell keeps its own identity.
 * The config, output, and error follow tiletrail_analyzer_create.
 */
int32_t tiletrail_analyzer_create_feeds(const tiletrail_config_t *config,
                                       tiletrail_timestamp_t start,
                                       uint32_t columns, uint32_t rows,
                                       tiletrail_analyzer_t **out,
                                       tiletrail_error_t *error);

#define TILETRAIL_HAS_GALLERY_HINT 1

/* Detect meetings with a gallery hint of up to 64 cells in display orientation.
 * Gallery identities follow content, including across cells. The hint applies
 * only where the frame contains evidence for the gallery. */
int32_t tiletrail_analyzer_create_gallery(const tiletrail_config_t *config,
                                         tiletrail_timestamp_t start,
                                         uint32_t columns, uint32_t rows,
                                         tiletrail_analyzer_t **out,
                                         tiletrail_error_t *error);

/* Constant-time progress of committed input. Expected end is an optional
 * exclusive source timestamp. Percent is -1 when the duration is unknown.
 * A live scan reports 0..99; a successful complete result is the terminal step.
 * State is READY before the first frame, ANALYZING after a successful push,
 * or NEEDS_REPLAY if a frame must be resubmitted. The end may equal the
 * pending frame, or follow it. Failure clears the output and sets percent
 * to -1. The last_timestamp field is meaningful when frames_processed > 0. */
enum { TILETRAIL_PROGRESS_READY = 0, TILETRAIL_PROGRESS_ANALYZING = 1,
       TILETRAIL_PROGRESS_NEEDS_REPLAY = 2, TILETRAIL_PROGRESS_COMPLETE = 3 };
typedef struct {
    uint64_t frames_processed;
    tiletrail_timestamp_t last_timestamp; /* valid when frames_processed > 0 */
    uint32_t state;
    int32_t percent;
} tiletrail_progress_t;
int32_t tiletrail_analyzer_progress(const tiletrail_analyzer_t *analyzer,
                                   const tiletrail_timestamp_t *expected_end,
                                   tiletrail_progress_t *out, tiletrail_error_t *error);

/* Borrow the frame and its len readable bytes only until this call returns.
 * Rotation is clockwise in degrees (0, 90, 180, 270).
 */
int32_t tiletrail_analyzer_push(tiletrail_analyzer_t *analyzer,
                               const tiletrail_frame_t *frame,
                               tiletrail_error_t *error);

/* Submit a frame that follows frames the caller skipped, typically the next
 * keyframe. The frame is committed only when every tracked region continues
 * from the last committed frame: the same appearances and identity state, no
 * event, and geometry within the tolerance of a move or resize event. The
 * result then reports the skipped interval as inferred coverage.
 *
 * TILETRAIL_COARSE_REFINE leaves the analyzer exactly as it was. Push the
 * skipped frames and then this frame with tiletrail_analyzer_push. A change
 * that appears and fully reverts between two committed frames is not seen.
 * The first frame of a scan is committed as a normal push. Frame, timestamp
 * and error rules match tiletrail_analyzer_push. Failure sets *outcome to
 * TILETRAIL_COARSE_REFINE.
 */
enum { TILETRAIL_COARSE_COMMITTED = 0, TILETRAIL_COARSE_REFINE = 1 };
int32_t tiletrail_analyzer_push_coarse(tiletrail_analyzer_t *analyzer,
                                      const tiletrail_frame_t *frame,
                                      uint32_t *outcome,
                                      tiletrail_error_t *error);

/* Measure and detect a frame ahead of its push, without changing the
 * analyzer. Several threads may prepare frames of one analyzer at the same
 * time, but not while another thread pushes to it. Preparation measures on
 * the CPU and fails for an analyzer that measures on the GPU. Failure sets
 * *out to NULL; submit that frame with tiletrail_analyzer_push instead.
 *
 * tiletrail_analyzer_push_prepared commits prepared frames in presentation
 * order with the result of pushing the frames themselves. A nonzero coarse
 * offers the frame as tiletrail_analyzer_push_coarse does and requires
 * outcome; otherwise outcome may be NULL. The call always consumes prepared.
 * Release a prepared frame that is never pushed with tiletrail_prepared_free,
 * which accepts NULL.
 */
typedef struct tiletrail_prepared tiletrail_prepared_t;
int32_t tiletrail_analyzer_prepare(const tiletrail_analyzer_t *analyzer,
                                  const tiletrail_frame_t *frame,
                                  tiletrail_prepared_t **out,
                                  tiletrail_error_t *error);
int32_t tiletrail_analyzer_push_prepared(tiletrail_analyzer_t *analyzer,
                                        tiletrail_prepared_t *prepared,
                                        uint32_t coarse, uint32_t *outcome,
                                        tiletrail_error_t *error);
void tiletrail_prepared_free(tiletrail_prepared_t *prepared);

/* Write an independently owned UTF-8 JSON snapshot, with no trailing NUL.
 * The end timestamp is exclusive. Further pushes are permitted afterwards.
 * The out struct must not own an unreleased buffer. Failure clears *out.
 * A successful buffer remains valid after the analyzer is freed.
 */
int32_t tiletrail_analyzer_result_json(const tiletrail_analyzer_t *analyzer,
                                      tiletrail_timestamp_t end,
                                      tiletrail_buffer_t *out,
                                      tiletrail_error_t *error);

/* A zero cursor starts a journal. Advance the durable cursor only when the
 * output chunk has been published. On resume, read chunks in journal order
 * into a fresh analyzer with the same configuration. Callbacks may not
 * re-enter the analyzer. Bytes are borrowed for each callback only.
 */
typedef struct {
    size_t observations, events, coarse;
} tiletrail_checkpoint_cursor_t;
typedef int32_t (*tiletrail_checkpoint_writer_t)(void *context, const uint8_t *data, size_t size);
typedef ptrdiff_t (*tiletrail_checkpoint_reader_t)(void *context, uint8_t *data, size_t capacity);
int32_t tiletrail_analyzer_checkpoint_write(const tiletrail_analyzer_t *analyzer,
                                           tiletrail_checkpoint_cursor_t *cursor,
                                           tiletrail_checkpoint_writer_t writer,
                                           void *context, tiletrail_error_t *error);
int32_t tiletrail_analyzer_checkpoint_read(tiletrail_analyzer_t *analyzer,
                                          tiletrail_checkpoint_cursor_t *cursor,
                                          tiletrail_checkpoint_reader_t reader,
                                          void *context, tiletrail_error_t *error);

/* Kind 0 is a result envelope with empty arrays. Kinds 1–6 are JSON batches
 * of observations, tracks, appearances, events, coverage and diagnostics.
 * Return zero to accept a batch or nonzero to stop.
 */
typedef int32_t (*tiletrail_result_chunk_t)(void *context, uint32_t kind,
                                           const uint8_t *data, size_t size);
int32_t tiletrail_analyzer_result_chunks(const tiletrail_analyzer_t *analyzer,
                                        tiletrail_timestamp_t end,
                                        tiletrail_result_chunk_t chunk,
                                        void *context, tiletrail_error_t *error);

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
