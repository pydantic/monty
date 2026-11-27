def make_cycle():
    holder = []
    generator = (holder for _ in [0])
    holder.append(generator)
    return generator


cycle = make_cycle()
next(cycle)
cycle = None

# A generator owns its exec globals even while its stack is on the VM.
namespace = {}
exec('generator = (value for value in [1, 2])', namespace)
next(namespace['generator'])
namespace = None

namespace = {}
exec('generator = (value for value in [1])', namespace)
list(namespace['generator'])
namespace = None

item = []

# The harness collects cycles, then rejects every unreachable heap object, so a
# leaked holder-generator cycle fails even though only the reachable item is named.
# ref-counts={'item': 1}
