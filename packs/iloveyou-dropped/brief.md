In May 2000 an email with the subject *ILOVEYOU* reached tens of millions of computers within hours. The attachment was this VBScript. Opening it ran the script, and the first thing `main()` did was copy the worm around the machine.

This is the actual worm source (a formatted, commented copy kept for study — see the link in the explainer). It does nothing here: it is shown as text, the way an analyst reads a sample. Nothing on this site runs it, and the Windows 98 / Outlook world it needed is long gone.

`main()` copies the script to **two** folders. Two copies go to the System folder; one goes to the Windows folder itself. **What filename does it use for the copy in the Windows folder?**

Answer with the filename.
