# Error Handling

How should errors during wit executions be handled?

To be clear, we are only investigating the case where wit functions are properly executed, and the execution fails due to an error in the wit function itself. 

Anything else outside of that is a "runtime" or "framework" bug that stops the execution of the runtime.

## Approaches

There are really three key problems that need solving:
1) what to do with the actor when a wit function call fails? Stop it, retry with backoff, sound the alarm, etc?
2) what to do with the message that caused the error? Retry it, discard it, etc?
3) how to communicate the error back to the sender of the message?

### Naive
The most simple and naive approach assumes that the wit functions deal with errors themselves fully and if an error bubbles up to the the actor executor, the executor stops. 

This means it's the responsibility of the wit implementer to decide if a new message should be commited to the step's inbox if the processing of that message produces an error. If an execution does not commit the message but also doesn't fail, the runtime will retry the message forever until it is commited.

This approach is what is currently implemented in the agent-os. The problem is that it offers not help with automated error handling or communication back to the sender.

### Automatic Catch and Retry

On top of the naive approach we can add facilities that retry a message, and communicate the errors back to the user. This could, for example, be implemented 

