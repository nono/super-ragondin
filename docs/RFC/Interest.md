Why I find the files synchronisation very interesting?
======================================================

I have officially worked full-time on Cozy-Desktop on a period of less than a
year, and it was several years ago (2015-2016). Since then, the development has
continued with other developers, and my role was more giving some advices,
helping for debugging some specific cases, and things like that. The client is
better now than it was in 2016, and by far. But I still think that its
architecture is not optimal and is the source of many bugs. I often have
thoughts on what the "right" architecture and algorithm should be, and I find
this problem very interesting. This note aims to explain why. There are two
sides of why a client that synchronize files between a desktop/laptop and the
cloud is a very hard problem: one is theoretical, and the other practical.

For the theoretical side, the problem is easy to explain: we have some
directories and files on a computer, and some directories and files on the
cloud, and we want to synchronize them while the user is still working with
them. But even after several years working on that, I can't write a
specification for that. Sure, we want an obvious property: if the user stops
making changes on the directories and files, and the client is running with
network, after long enough, the client will stop doing write operations, and
then the files and directories on the computer will be the same of the ones in
the cloud. But a very stupid client that deletes everything on both the local
disk and in the cloud satisfies this condition. So, we need another condition.
This condition might be that no content is lost. But, I didn't find a way to
formalize that, and I'm not even sure that it is really what people expect.
Let's take an example: if we have a directory synchronized, and while the
client is offline, a file is added inside this directory on the cloud, and the
directory is deleted on the local disk, what should do the client when it comes
back online? In my experience, what the users expect is often that the
directory is put in the trash in the local disk, and the file is not uploaded
to the cloud. The file is not really lost, as the user can still recover it
from the trash. But, in any case, whether you consider that putting something
in the trash is losing it or not, whether you consider that the client should
put things in the trash or deletes them permanently, you still have a problem
for which it is very hard to write an interesting specification. By
interesting, I mean that it should match the people expectation (even if not
perfectly), and it should not be defeated by a stupid client.

In particular, specification often says that, for a set on inputs, there is one
correct solution. In our case, I don't think so. For example, if we have two
directories A and B synchronized, and while the client is offline, A is moved
into B on the local disk, and B is moved into A on the cloud, when the client
comes back online, what should it do? We can't have both A in B, and B in A,
file system prevents that. So, the client can synchronize to make A in B in
both places, or B in A in both places, and I think the two behaviors are
correct. And there other examples like file conflicts where we have two or more
behaviors that are corrects. We can probably add a synthetic rule, to say that
what is the right behavior in that case (like the first change wins, or the
change made in the cloud wins), but this rule is no longer derived from what
people expects, which means that the specification with this rule whould reject
clients with a behavior that people may find totally find. So, such a
specification could only be used to build a new client, not to build a test
suite for existing clients.

Let's switch to the other side, the practical one. The file systems have messy
interfaces. You don't write a file, you open a file descriptor for it, and you
move the file descriptor to the right place, and you write at this place, and
finally, you close the file descriptor: more complex, and not atomic! And, what
is a file in the first place? Something with a unique identifier called the
inode number you might be tempted to say. But, on Linux, the file systems reuse
the inode numbers: when a file is deleted, and another is created, they may have
the same inode number. And, for a user, clicking on save in a software like
Office or vim doesn't make it a new file, it just changes the content. But, in
practice, with the lack of atomic operations provided by the file systems, those
software really creates a new file, write the new content inside it, and then
moved it over the old file.

Oh, and let's talk about how to be notified when a file or directory is
changed. Each OS has its own system: windows has ReadDirectoryChangesW, masOS
has FsEvents, and Linux has inotify. Each system has their own behaviors and
quirks. And none of the three is very good: you still have to make a lot of
calls to `stat(2)` to understand what were the changes. And you have no way to
know if a change has been by the client itself or by another program.

Yeah, the files are a messy world, with lots of asynchronous things. And, with
the watchers, we can say 3 messy worlds, as we need to have specific code for
macOS, Linux, and Windows.

Appendix about the specification
--------------------------------

If you want to think about a specification, you may want to start with those definitions:

- a node can be a file or a directory, with a unique identifier, a name and a
  parent (which must be a directory, or the root)
- a directory doesn't have other attributes
- a file also has a content
- a file system is tree of nodes + a root.

We can see the cloud and local disk as two file systems, and the client must
synchronize them. The specification must define what "synchronize" means.

The user will do some operations from this set at any time:

- start the client
- stop the client
- going online (the client has network and can talk with the cloud)
- going offline (the client can no longer talk with the cloud)
- create a file
- create a directory
- rename a file
- rename a directory
- move a file to another place (a directory or the root)
- move a directory to another place (which is not inside it, to preserve the tree stucture)
- change the content of a file
- delete a file
- delete a directory.

The file and directory operations can be made on the local disk or in the cloud.

*Note:* in practice, there are symlinks, and trash, and mtime/ctime/atime, and
permissions. But I don't think they are in the core of the specification, they
can be added later if we want to.

The client receives an event when any of those operations is performed if it is
running. And it can do those operations:

- list what is in a directory
- ask to be notified of the future changes from a file system
- ask information for a path (like `stat(2)`, it returns the identifier, type,
  name, and parent identifier)
- upload a file from the local disk to the cloud
- download a file from the cloud to the local disk
- rename a file
- rename a directory
- move a file
- move a directory
- delete a file
- delete a directory.

In the real world, all the operations are asynchronous. But, you can start with
synchronous operations if it helps you writing a specification.

For the notifications from a file system, it is vague, but on purpose, as I
don't know what should be the right formalism here.

We may also want to add those 2 operations for the client:

- persist its state to a database
- reload its state from a database.
