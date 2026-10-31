"""Generate original English dialogue for the subtitle-search benchmark."""

from pathlib import Path

# Original dialogue: short turns, names, punctuation and occasional accented words.
DIALOGUE = (
    'Maya, did the night train arrive yet?',
    'The board says platform four. We still have a few minutes.',
    'Please keep the blue notebook where I can see it.',
    'I left it at the café beside the station.',
    'Alex Rivera called. The spare key is under the flowerpot.',
    'That would have been useful to know before we climbed the fence.',
    "Wait here. I'll ask whether they found your bag.",
    'It has a green ribbon, a broken zip, and a map of Zürich.',
    'Are you sure this is the right address?',
    'Yes. Alex Rivera wrote it on the back of the receipt.',
    'Then why does the sign say the office moved last Tuesday?',
    'Because neither of us remembered to read the other side.',
    'We can take the bus, or walk along the river.',
    "Let's walk. I need to explain what happened at the meeting.",
    "No, the parcel wasn't empty. There was a photograph inside.",
    'A photograph of whom? And why send it to us?',
    'Alex Rivera might recognize the house in the background.',
    "Fine. But first we're getting something to eat.",
    'The soup is hot. Be careful with that bowl.',
    'Thanks. Next time, remind me to bring an umbrella.',
    'Did you hear that? Someone is knocking at the back door.',
    'Probably the neighbour. She promised to return the ladder.',
    "Tell Alex Rivera we'll meet at the bridge before sunset.",
    "All right. This time I'll write down which bridge.",
)


def make_text():
    cues = []
    for index in range(480):
        start = index * 4
        minute, second = divmod(start, 60)
        end_minute, end_second = divmod(start + 3, 60)
        cues.append(
            f'{index + 1}\n'
            f'00:{minute:02}:{second:02},000 --> 00:{end_minute:02}:{end_second:02},000\n'
            f'{DIALOGUE[index % len(DIALOGUE)]}\n\n'
        )
    return ''.join(cues).rstrip('\n') + '\n'


if __name__ == '__main__':
    Path(__file__).with_name('subtitles_en.txt').write_text(make_text(), encoding='utf-8', newline='\n')
