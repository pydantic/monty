def make_vfs_importer(read_source, bindings):
    """Return a loader with a fresh cache; assign it to `__monty_import__` to enable imports.

    `read_source` must use the approved virtual filesystem, not the host importer.
    Native dotted imports walk exported attributes; package initialization is explicit.
    """
    reserved = {'__monty_import__', '__name__', '__file__'}
    if any(name in reserved for name in bindings):
        raise ValueError('bindings contain a reserved module name')
    cache = {}
    loading = []
    first_chars = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ_'
    rest_chars = first_chars + '0123456789'

    class Library:
        pass

    def load(name):
        parts = name.split('.')
        if not all(part and part[0] in first_chars and all(c in rest_chars for c in part) for part in parts):
            raise ImportError('invalid virtual module name: ' + name)
        if name in cache:
            return cache[name]
        if name in loading:
            raise ImportError('circular virtual import: ' + ' -> '.join(loading + [name]))
        path = '/modules/' + '/'.join(parts) + '.py'
        loading.append(name)
        try:
            try:
                source = read_source(path)
            except FileNotFoundError:
                raise ModuleNotFoundError("No virtual module named '" + name + "'") from None
            namespace = dict(bindings)
            namespace['__name__'] = name
            namespace['__file__'] = path
            namespace['__monty_import__'] = load
            exec(source, namespace)
            module = Library()
            # Instance attributes are export snapshots, not live module globals.
            for key, value in namespace.items():
                if not key.startswith('_') and key not in bindings:
                    setattr(module, key, value)
            cache[name] = module
            return module
        finally:
            loading.pop()

    return load
