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

typedef struct pdf_document pdf_document;
typedef struct mupdf_error mupdf_error_t;
void mupdf_save_error(fz_context *, mupdf_error_t **);
void pdf_select_layer_config(fz_context *, pdf_document *, int);
void pdf_enable_layer(fz_context *, pdf_document *, int, int);

void review_set_layers(fz_context *ctx, pdf_document *document, size_t count,
    const int *indices, const int *enabled, mupdf_error_t **errptr)
{
    fz_try(ctx)
    {
        pdf_select_layer_config(ctx, document, -1);
        for (size_t i = 0; i < count; ++i)
            pdf_enable_layer(ctx, document, indices[i], enabled[i]);
    }
    fz_catch(ctx)
        mupdf_save_error(ctx, errptr);
}
