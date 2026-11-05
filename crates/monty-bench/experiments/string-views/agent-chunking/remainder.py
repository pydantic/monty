def chunk_document(text, max_chars=2000, overlap=256):
    chunks = []
    offset = 0
    while text:
        if len(text) <= max_chars:
            chunks.append((offset, text))
            break
        end = text.rfind('\n\n', max_chars // 2, max_chars)
        if end < 0:
            end = text.rfind(' ', max_chars // 2, max_chars)
        if end < 0:
            end = max_chars
        chunks.append((offset, text[:end]))
        advance = end - overlap
        text = text[advance:]
        offset += advance
    return chunks
