/* Additional bindings for the MuPDF 1.27.2 shipped by mupdf-sys 0.8.0.
 * All throwing native calls stay inside a C exception boundary.
 */
#include "mupdf/fitz/stream.h"

typedef struct fz_document fz_document;
fz_document *fz_open_document_with_stream(fz_context *, const char *, fz_stream *);

fz_document *review_open_shared(fz_context *ctx, const unsigned char *data,
    size_t len, const char *magic, void *owner, fz_stream_drop_fn *drop,
    int *code, char message[512])
{
    fz_stream *stream = NULL;
    fz_document *document = NULL;
    fz_var(stream);
    fz_var(document);
    fz_try(ctx)
    {
        stream = fz_open_memory(ctx, data, len);
        /* fz_open_memory has no owner. Attach it before any further throwing
         * call so native document/page clones keep the bytes alive as well.
         * Every open has its own stream cursor over the immutable bytes.
         */
        stream->state = owner;
        stream->drop = drop;
        document = fz_open_document_with_stream(ctx, magic, stream);
    }
    fz_always(ctx)
        fz_drop_stream(ctx, stream);
    fz_catch(ctx)
    {
        if (!stream)
            drop(ctx, owner);
        *code = fz_caught(ctx);
        snprintf(message, 512, "%s", fz_caught_message(ctx));
    }
    return document;
}
