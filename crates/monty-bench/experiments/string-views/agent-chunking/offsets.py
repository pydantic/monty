def chunk_document(text, max_chars=2000, overlap=256):
    chunks = []
    length = len(text)
    offset = 0
    while offset < length:
        if length - offset <= max_chars:
            chunks.append((offset, text[offset:]))
            break
        end = text.rfind('\n\n', offset + max_chars // 2, offset + max_chars)
        if end < 0:
            end = text.rfind(' ', offset + max_chars // 2, offset + max_chars)
        if end < 0:
            end = offset + max_chars
        chunks.append((offset, text[offset:end]))
        offset = end - overlap
    return chunks
