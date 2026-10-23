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

typedef struct fz_pixmap fz_pixmap;
typedef struct fz_colorspace fz_colorspace;
typedef struct fz_separations fz_separations;
typedef struct fz_device fz_device;
fz_colorspace *fz_device_rgb(fz_context *);
fz_pixmap *fz_new_pixmap_with_bbox_and_data(fz_context *, fz_colorspace *,
    fz_irect, fz_separations *, int, unsigned char *);
fz_device *fz_new_draw_device(fz_context *, fz_matrix, fz_pixmap *);
void fz_close_device(fz_context *, fz_device *);

fz_pixmap *review_rgba_pixmap(fz_context *ctx, fz_irect bbox,
    unsigned char *samples, mupdf_error_t **errptr)
{
    fz_pixmap *pixmap = NULL;
    fz_var(pixmap);
    fz_try(ctx)
        pixmap = fz_new_pixmap_with_bbox_and_data(ctx, fz_device_rgb(ctx),
            bbox, NULL, 1, samples);
    fz_catch(ctx)
        mupdf_save_error(ctx, errptr);
    return pixmap;
}

fz_device *review_draw_device(fz_context *ctx, fz_pixmap *pixmap,
    fz_matrix transform, mupdf_error_t **errptr)
{
    fz_device *device = NULL;
    fz_var(device);
    fz_try(ctx)
        device = fz_new_draw_device(ctx, transform, pixmap);
    fz_catch(ctx)
        mupdf_save_error(ctx, errptr);
    return device;
}

void review_close_draw_device(fz_context *ctx, fz_device *device,
    mupdf_error_t **errptr)
{
    fz_try(ctx)
        fz_close_device(ctx, device);
    fz_catch(ctx)
        mupdf_save_error(ctx, errptr);
}
