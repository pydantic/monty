# The PyScript antigravity example, unchanged apart from these lines, which
# replace its five imports. `random`, `pydom` and `DOMParser` are host objects,
# `open_url` and `set_interval` are host functions, and main.ts decides what
# each of them is allowed to do.


class Antigravity:
    url = "./antigravity.svg"

    def __init__(self, target=None, interval=10, append=True, fly=False):
        if isinstance(target, str):
            # get element with target as id
            self.target = pydom[f"#{target}"][0]
        else:
            self.target = pydom["body"][0]

        doc = DOMParser.new().parseFromString(
            open_url(self.url).read(), "image/svg+xml"
        )
        self.node = doc.documentElement

        if append:
            self.target.append(self.node)
        else:
            self.target._js.replaceChildren(self.node)

        self.xoffset, self.yoffset = 0, 0
        self.interval = interval

        if fly:
            self.fly()

    def fly(self):
        set_interval(self.move, self.interval)

    def move(self):
        char = self.node.getElementsByTagName("g")[1]
        char.setAttribute("transform", f"translate({self.xoffset}, {-self.yoffset})")
        self.xoffset += random.normalvariate(0, 1) / 20
        if self.yoffset < 50:
            self.yoffset += 0.1
        else:
            self.yoffset += random.normalvariate(0, 1) / 20

_auto = Antigravity(append=True)
fly = _auto.fly
